package fsdfeed

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// 本文件钉的是 Feed 的 HTTP 那一侧：响应头阶段的时限、每条返回路径都要关 body、
// 以及空闲时限在流跑着的时候能不能被安全改动。三件事都在 feed_test.go 的
// 假 SSE 服务端够不着的地方——那些测试全都假定 Do() 会返回、body 会被读完。

// TestAServerThatWithholdsResponseHeadersDoesNotWedgeTheFeed 钉住第三个 wedge。
//
// 形状：上游把 TCP/TLS 握完、把请求收下，然后**一个响应头都不发**。一个反向
// 代理在等一个已经挂掉的后端时正是这样，一个被防火墙半吞的连接也是。
//
// 为什么别的东西救不了它：空闲看门狗是在 `Do()` **返回之后**才上弦的，所以在
// 这个状态下它一秒都没走过；`bufio.Scanner` 还没有拿到 body；ctx 只有进程退出
// 时才会被取消。于是 `Feed.Run` 停在那一行到进程结束，degraded 停在上一次的值，
// 服务器拿着过期的位置继续满怀信心地做射程过滤——而日志里一个字都没有。
//
// 断言必须是"stream() 在时限的几倍之内返回了"，不是"它返回了错误"：不修的话
// 它**不会**返回，所以任何对返回值的断言都到不了。
func TestAServerThatWithholdsResponseHeadersDoesNotWedgeTheFeed(t *testing.T) {
	restore := responseHeaderTimeout.Set(500 * time.Millisecond)
	defer responseHeaderTimeout.Set(restore)

	// 看门狗调到远大于响应头时限：它本来就够不着这条路径，调大只是把
	// "也许是看门狗顺手救了它"这个解释也排除掉。
	restoreIdle := feedIdleTimeout.Set(60 * time.Second)
	defer feedIdleTimeout.Set(restoreIdle)

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// 什么都不写——连 WriteHeader 都不调。响应头因此一直停在服务端这边。
		<-r.Context().Done()
	}))
	defer srv.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel() // defer 是 LIFO：它排在 srv.Close() 前面跑，否则失败时 Close 会等那个还挂着的 handler

	// NewFeed 要在 Set 之后调：客户端是在这里按当前值造出来的。
	f := NewFeed(srv.URL)
	done := make(chan error, 1)
	go func() { done <- f.stream(ctx) }()

	select {
	case err := <-done:
		if err == nil {
			t.Fatal("stream() returned nil for a server that never sent a response header")
		}
		t.Logf("stream() gave up with: %v", err)
	case <-time.After(5 * time.Second):
		t.Fatal("stream() is still waiting for a response header ten times past the timeout; with no ResponseHeaderTimeout on the client this wedges Feed.Run until the process exits — the idle watchdog cannot help, it is only armed after Do() returns")
	}
}

// TestTheFeedClientKeepsTheTimeoutsItInherits 是上一条的另一半。
//
// 堵一个 wedge 最省事的写法是 `&http.Client{Transport: &http.Transport{
// ResponseHeaderTimeout: …}}`——它能让上面那条测试变绿，同时**丢掉**
// DefaultTransport 里已经有的拨号超时和 TLS 握手超时。后者的零值是"不超时"，
// 于是响应头阶段的 wedge 被堵上了，而更早的 TLS 握手阶段开出一个新的，
// 症状一模一样。一个没有任何测试看得见的净损失。
//
// 顺带钉住 Client.Timeout 必须是零：它是对**整个请求**计时的，而这个 body 是
// 一条永不结束的流——设上它，feed 会每隔 Timeout 被从中间掐断一次。
func TestTheFeedClientKeepsTheTimeoutsItInherits(t *testing.T) {
	c := newFeedClient()
	if c.Timeout != 0 {
		t.Fatalf("Client.Timeout = %v, want 0 — it times the whole request including the body, and this body is a stream that never ends", c.Timeout)
	}
	tr, ok := c.Transport.(*http.Transport)
	if !ok {
		t.Fatalf("Transport is %T, want *http.Transport", c.Transport)
	}
	if tr.ResponseHeaderTimeout != responseHeaderTimeout.Get() {
		t.Fatalf("ResponseHeaderTimeout = %v, want %v", tr.ResponseHeaderTimeout, responseHeaderTimeout.Get())
	}
	if tr.TLSHandshakeTimeout == 0 {
		t.Fatal("TLSHandshakeTimeout is 0 (meaning no timeout): the transport was built from scratch instead of cloning DefaultTransport, so plugging the response-header wedge opened an identical one in the TLS handshake")
	}
	if tr.DialContext == nil && tr.Dial == nil {
		t.Fatal("the transport has no DialContext: DefaultTransport's 30s dial timeout was dropped, so a black-holed address wedges the feed before it ever gets to the headers")
	}
	if tr.Proxy == nil {
		t.Fatal("the transport has no Proxy function: ProxyFromEnvironment was dropped, and a deployment behind a proxy would silently bypass it")
	}
}

// countingBody 是一个记着自己被 Close 了几次的响应 body。
type countingBody struct {
	r      io.Reader
	closes *atomic.Int32
}

func (b *countingBody) Read(p []byte) (int, error) { return b.r.Read(p) }

func (b *countingBody) Close() error {
	b.closes.Add(1)
	return nil
}

// errReader 先吐出 data，然后报一个**不是 io.EOF** 的错误——一条被中途
// 重置的连接就是这个样子，bufio.Scanner 会把它经 sc.Err() 报上来。
type errReader struct {
	data string
	err  error
	done bool
}

func (r *errReader) Read(p []byte) (int, error) {
	if !r.done && r.data != "" {
		n := copy(p, r.data)
		r.data = r.data[n:]
		if r.data == "" {
			r.done = true
		}
		return n, nil
	}
	return 0, r.err
}

// stubTransport 不碰网络，直接造一个响应，body 是可数的。
type stubTransport struct {
	status int
	body   io.Reader
	closes atomic.Int32
}

func (tr *stubTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	return &http.Response{
		StatusCode: tr.status,
		Status:     fmt.Sprintf("%d %s", tr.status, http.StatusText(tr.status)),
		Proto:      "HTTP/1.1",
		Header:     http.Header{},
		Body:       &countingBody{r: tr.body, closes: &tr.closes},
		Request:    req,
	}, nil
}

// TestEveryReturnPathClosesTheResponseBody 钉住 `defer resp.Body.Close()`。
//
// 为什么要单独钉：这条 defer 在 stream() 里只占一行，而它保护的是**三条不会把
// 响应读空的返回路径**——状态码不对、连续失败触发、扫描出错。net/http 只在 body
// 被读完或者被关掉之后才会把那条连接放回池子，所以漏掉它的表现是"连上游故障
// 之后，文件描述符每 5 秒（reconnectDelay）漏一个"，而不是任何一条日志。
//
// 这条测试不涉及任何时序：假 RoundTripper 直接给一个计数 body，三条路径各跑一次，
// 每一次都断言 closed == 1。删掉那条 defer，三条全红。
func TestEveryReturnPathClosesTheResponseBody(t *testing.T) {
	// 五条解不开的事件正好打满 maxConsecutiveParseFailures。
	var unusable strings.Builder
	for i := 0; i < maxConsecutiveParseFailures; i++ {
		unusable.WriteString("event: snapshot\ndata: {\n\n")
	}

	for _, tc := range []struct {
		name   string
		status int
		body   io.Reader
		why    string
	}{
		{
			name:   "the status is not 200",
			status: http.StatusNotFound,
			body:   strings.NewReader(""),
			why:    "a 404 returns before a single byte of the body is read",
		},
		{
			name:   "too many unusable events in a row",
			status: http.StatusOK,
			body:   strings.NewReader(unusable.String()),
			why:    "recordFailure trips and stream() returns with the body half-read",
		},
		{
			name:   "the stream errors mid-read",
			status: http.StatusOK,
			body:   &errReader{data: "event: snapshot\n", err: errors.New("connection reset by peer")},
			why:    "sc.Err() is non-nil and stream() returns straight out of the scan loop",
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			tr := &stubTransport{status: tc.status, body: tc.body}
			f := NewFeed("http://feed.invalid/v1/events")
			f.client = &http.Client{Transport: tr}

			if err := f.stream(context.Background()); err == nil {
				t.Fatalf("stream() returned nil, want an error — %s", tc.why)
			}
			if got := tr.closes.Load(); got != 1 {
				t.Fatalf("the response body was closed %d times, want exactly 1: %s, so nothing else will ever close it and net/http keeps the connection (and its fd) out of the pool forever — Run retries every %v",
					got, tc.why, reconnectDelay)
			}
		})
	}
}

// TestTheIdleTimeoutIsSafeToChangeWhileAFeedIsRunning 钉住 feedIdleTimeout
// 必须是 atomicDuration 而不是裸 var。
//
// **这条测试只在 `-race` 下有牙**，这也正是它该在的地方：CI 跑的就是 `-race`
// （见仓库的工作流），而这一类缺陷除了竞态检测器没有别的东西看得见。
//
// 形状是真实的：写的是测试 goroutine（每条要改时限的测试都这么干），读的是
// Run/stream 的 goroutine，两者之间没有任何 happens-before 边。今天靠的是
// "每条测试都先 cancel 再 <-done 才还原"这个纪律，而纪律不是机制——下一条
// 忘了等的测试会把整包染红，而那种红会盖住真正的并发缺陷。
//
// 数量是特意堆上去的（50 次 stream 入口，配几百到几千次写）：竞态检测器只
// 报告它**确实看到过**的那一对访问，一两次碰撞是碰运气，几百次不是。实测把
// atomicDuration 换回裸字段，默认和 GOMAXPROCS=1 各 3/3 都报 DATA RACE。
func TestTheIdleTimeoutIsSafeToChangeWhileAFeedIsRunning(t *testing.T) {
	orig := feedIdleTimeout.Get()
	defer feedIdleTimeout.Set(orig)

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		fmt.Fprintf(w, "event: snapshot\ndata: %s\n\n", realSnapshotEvent)
		// 立刻返回，于是 stream() 很快走完一轮，下一轮又会重新读一次时限。
	}))
	defer srv.Close()

	f := NewFeed(srv.URL)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// 轮数固定、写方跟着读方走，而不是反过来：写 200 次原子量只要几微秒，
	// 让读方去追它的话，一次测试里 stream() 可能只进去过一轮——那就又回到
	// 碰运气了（第一版正是这样，整条测试 0.00 秒跑完）。
	const rounds = 50
	reading := make(chan struct{})
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		defer close(reading)
		for i := 0; i < rounds; i++ {
			// 直接调 stream 而不是 Run：Run 每轮之间要睡 reconnectDelay（5 秒），
			// 一次测试跑不了几轮。
			_ = f.stream(ctx)
		}
	}()

	writes := 0
	for {
		select {
		case <-reading:
			wg.Wait()
			if writes < rounds {
				t.Fatalf("only %d writes landed against %d stream() entries; the two sides never really overlapped", writes, rounds)
			}
			goto checked
		default:
		}
		feedIdleTimeout.Set(time.Duration(writes%60+1) * time.Second)
		writes++
		// 睡一下，不要空转。单核（CI 会专门跑一遍 GOMAXPROCS=1）上一个不让步的
		// 写循环会把读方饿住：实测那时这条测试从 0.02 秒涨到 4.96 秒、写了三千万次，
		// 还把同一个包里几条靠时间窗口的测试一起拖红。runtime.Gosched() 也不够
		// ——没有别的可运行 goroutine 时它立刻就回来了。睡一微秒（实际粒度几十
		// 微秒）之后写的次数仍以万计，竞态检测器要的那一对访问远远够。
		time.Sleep(time.Microsecond)
	}
checked:
	t.Logf("%d stream() entries against %d concurrent writes", rounds, writes)

	// 前提：这几百轮里 feed 真的连上过，也就是那个读**真的被执行到了**。
	// 否则这条测试只是证明了"没跑过的代码不会有竞争"。
	if f.Degraded() {
		t.Fatal("the feed never came up during the whole run, so stream() may never have reached the line that reads feedIdleTimeout and this test proves nothing")
	}
}

// TestTheSubscriptionSendsABrowserShapedUserAgent 钉住订阅请求带着 User-Agent。
//
// **数据源前面挡着 Cloudflare，非浏览器形态的 UA 一律 403。** 不带这个头的话，
// net/http 会替我们填上 `Go-http-client/1.1`——而 403 在这一侧的样子是
// stream() 立刻返回错误、Run 每 5 秒重来一次、日志里只有一行 `fsd feed dropped`，
// 射程过滤永久降级成全球互通。can-audio 的 `server/ATIS/request.py` 为此写过
// 一整段注释，本仓库 Rust 侧的两处（can-voice-atis、can-voice-datafeed）也各自
// 设了同一形状的 UA；这一处是最后一个漏掉的。
func TestTheSubscriptionSendsABrowserShapedUserAgent(t *testing.T) {
	got := make(chan string, 1)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		select {
		case got <- r.UserAgent():
		default:
		}
		// 什么都不写就返回：200 加一个空 body，stream() 读到 EOF 正常退出。
		// 这条测试只关心请求头。
	}))
	defer srv.Close()

	f := NewFeed(srv.URL)
	_ = f.stream(context.Background())

	select {
	case ua := <-got:
		if ua != userAgent {
			t.Fatalf("User-Agent = %q, want %q", ua, userAgent)
		}
	default:
		t.Fatal("the feed never issued a request, so this test proves nothing about its headers")
	}
}

// TestTheUserAgentDoesNotLookLikeALibraryDefault 是上一条的另一半：
// 头**设了**不等于设对了。
//
// 和 Rust 侧 `crates/can-voice-atis/src/datafeed.rs` 的
// `the_user_agent_does_not_look_like_a_library_default` 是同一条断言，
// 刻意照抄那张拒绝名单——三处实现共用一条规则，就该共用同一张名单。
func TestTheUserAgentDoesNotLookLikeALibraryDefault(t *testing.T) {
	if !strings.HasPrefix(userAgent, "Mozilla/") {
		t.Fatalf("User-Agent = %q, want something that starts with Mozilla/", userAgent)
	}
	for _, banned := range []string{"Go-http-client", "reqwest", "python-requests", "curl"} {
		if strings.Contains(userAgent, banned) {
			t.Fatalf("User-Agent = %q contains %q, which is on Cloudflare's reject list", userAgent, banned)
		}
	}
}
