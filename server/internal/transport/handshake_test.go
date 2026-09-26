package transport

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"errors"
	"net"
	"sync/atomic"
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/auth"
	"github.com/JianyueLab-Org/can-voice/server/internal/control"
	"github.com/JianyueLab-Org/can-voice/server/internal/router"
	"github.com/quic-go/quic-go"
)

// 本文件直接调 handshake()，不经过真的 QUIC 连接。
//
// 这样做只为了两件用真连接做不到的事：让 READY 的写**失败**，以及让顶号的
// 断连**慢下来**。两个假件都只嵌一个 nil 接口、只实现这条路径真正会碰的方法——
// 碰到别的会 nil 解引用 panic，而那正是想知道的：这条路径不该碰别的东西。

// memStream 是一条假的控制流：读出预先塞好的字节，写进一个缓冲区。
type memStream struct {
	quic.Stream // 只为满足类型；handshake 只会调 Read 和 Write。
	in          *bytes.Reader
	out         bytes.Buffer
	failWrite   bool

	// 两个截止时间只是**记下来**：这条假流上没有真的 I/O 可以打断，
	// 所以它们打不断任何东西——真的超时由走真 QUIC 连接的那几条测试钉。
	// 记下来是为了 armHandshakeDeadlines 那条测试，它要问的是"两侧是不是
	// 成对地上了弦、又成对地撤了"，而那个问题在真连接上根本看不见。
	readDeadline  time.Time
	writeDeadline time.Time
}

func (s *memStream) Read(p []byte) (int, error) { return s.in.Read(p) }

func (s *memStream) SetReadDeadline(t time.Time) error  { s.readDeadline = t; return nil }
func (s *memStream) SetWriteDeadline(t time.Time) error { s.writeDeadline = t; return nil }

func (s *memStream) Write(p []byte) (int, error) {
	if s.failWrite {
		// 真实世界里的对应物：对端把 initial_max_stream_data 通告成 0，
		// 或者连接在验签和写 READY 之间死掉了。
		return 0, errors.New("stream write failed")
	}
	return s.out.Write(p)
}

// stubConn 记录断连有没有发生，并且可以让断连阻塞住。
type stubConn struct {
	quic.Connection // 只为满足类型；handshake 期间一个方法都不会被调到，
	// 它只是被两个闭包捕获。
	closed atomic.Bool
	// block 非 nil 时 CloseWithError 会等它——用来模拟 quic-go 的
	// CloseWithError 末尾那句 `<-s.ctx.Done()`（它等到连接主循环真的退出）。
	block chan struct{}
	// stream 非 nil 时 AcceptStream 返回它。只有直接调 handleConn 的测试要用
	// （goroutine 泄漏那条）——handshake 自己收的是已经开好的流。
	stream quic.Stream
}

func (c *stubConn) SendDatagram([]byte) error { return nil }

// The production handshake rejects peers that did not negotiate QUIC
// datagrams. The in-memory connection is used by tests that exercise the
// later handshake and teardown paths, so model the capability explicitly
// instead of delegating ConnectionState to the nil embedded interface.
func (c *stubConn) ConnectionState() quic.ConnectionState {
	return quic.ConnectionState{SupportsDatagrams: true}
}

// ReceiveDatagram 只是挂住：handleConn 起的 readDatagrams goroutine 会调它，
// 而 stubConn 里嵌的 quic.Connection 是 nil，不实现它就是空指针 panic——
// 那个 panic 还发生在**另一个 goroutine 上**，整个测试进程跟着走。
func (c *stubConn) ReceiveDatagram(ctx context.Context) ([]byte, error) {
	<-ctx.Done()
	return nil, ctx.Err()
}

func (c *stubConn) AcceptStream(ctx context.Context) (quic.Stream, error) {
	if c.stream == nil {
		<-ctx.Done()
		return nil, ctx.Err()
	}
	return c.stream, nil
}

func (c *stubConn) RemoteAddr() net.Addr {
	return &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1), Port: 1}
}

func (c *stubConn) CloseWithError(quic.ApplicationErrorCode, string) error {
	c.closed.Store(true)
	if c.block != nil {
		<-c.block
	}
	return nil
}

func testKeys(t *testing.T) (ed25519.PublicKey, ed25519.PrivateKey) {
	t.Helper()
	pub, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatalf("GenerateKey: %v", err)
	}
	return pub, priv
}

// helloStream 造一条已经装好一帧 HELLO 的假控制流。
func helloStream(t *testing.T, priv ed25519.PrivateKey, cid string, failWrite bool) *memStream {
	t.Helper()
	tok, err := auth.Sign(priv, auth.Claims{
		CID: cid, Rating: 5, MaxTX: 8, Exp: time.Now().Add(time.Minute).Unix(),
	})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	b, err := control.Encode(&control.Hello{Token: tok, Client: "test/1", Proto: 1})
	if err != nil {
		t.Fatalf("Encode: %v", err)
	}
	var framed bytes.Buffer
	if err := control.WriteFrame(&framed, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	return &memStream{in: bytes.NewReader(framed.Bytes()), failWrite: failWrite}
}

// TestArmingTheHandshakeDeadlinesCoversBothDirections 钉住那两行必须成对。
//
// 握手期间读和写各有一种 slowloris：一言不发的对端占着一条连接和一个 goroutine，
// 而一个把 initial_max_stream_data_bidi_remote 通告成 0 的对端让服务端的第一次
// Write 永远阻塞在流控上。两侧只上一侧的弦，剩下那一侧就是敞着的。
//
// 撤销同样要成对，而且漏撤的那一半更难查：握手那个**一次性**的写时限留在一条
// 长连接上早晚会过期，然后掐死一条完全健康的会话，症状发生在十秒之后、
// 和任何一次操作都对不上。
//
// 这两件事在真 QUIC 连接上看不见（截止时间没有读回来的入口），所以它们此前
// 一行钉子都没有：把 handleConn 里 SetWriteDeadline 那一行删掉，整套测试照绿。
func TestArmingTheHandshakeDeadlinesCoversBothDirections(t *testing.T) {
	st := &memStream{}
	at := time.Unix(1757000000, 0)

	armHandshakeDeadlines(st, at)
	if !st.readDeadline.Equal(at) {
		t.Fatalf("read deadline = %v, want %v — a peer that opens the stream and says nothing must not be able to hold a goroutine open forever", st.readDeadline, at)
	}
	if !st.writeDeadline.Equal(at) {
		t.Fatalf("write deadline = %v, want %v — a peer that advertises a zero stream window blocks the server's first Write on flow control, which is the same slowloris one step along", st.writeDeadline, at)
	}

	armHandshakeDeadlines(st, time.Time{})
	if !st.readDeadline.IsZero() {
		t.Fatalf("read deadline = %v after disarming, want the zero time — a controller may say nothing for minutes", st.readDeadline)
	}
	if !st.writeDeadline.IsZero() {
		t.Fatalf("write deadline = %v after disarming, want the zero time — leaving the handshake's one-shot deadline on a long-lived connection kills a healthy session ten seconds later, with nothing to connect it to", st.writeDeadline)
	}
}

// waitClosed 等断连回调被调到（它是异步的），返回有没有等到。
func waitClosed(c *stubConn, d time.Duration) bool {
	deadline := time.Now().Add(d)
	for time.Now().Before(deadline) {
		if c.closed.Load() {
			return true
		}
		time.Sleep(5 * time.Millisecond)
	}
	return false
}

// TestAFailedReadyWriteDoesNotLeakTheSession 钉住 READY 写不出去时会话被摘掉。
//
// 会话是在写 READY **之前**登记进 router 的（它得先有 id 才能写进 READY），
// 所以写失败那条路径必须自己收拾。漏掉的话，这条连接已经没了，而 router 里还有
// 一条以它为名的会话，占着这个 CID、挂在扇出索引上。
//
// 两句断言，一句直接、一句按生产的形状走：
//
//  1. **会话数没有涨。** 直接、同步、不带任何时长。断言的是数量而不是"某个具体
//     id 不存在"——newSessionID 是进程全局递增、从不重置的，所以整包一起跑的
//     时候任何一个具体 id 的断言都恒真（这正是 C-2 点名的那个空过）。
//  2. **同一个 CID 下一次登录没有找到东西可顶。** 这一句走的是顶号那条真路径，
//     而顶号会调被顶方的 Close 回调——所以"victim 有没有被 Close"就是"它是不是
//     还在表里"。
//
// 第二句此前是 `waitClosed(victim, 300ms)`：一条**通过条件就是超时**的否定断言。
// 实测它不是抖动源（200 轮最差 1.015 毫秒，296 倍余量），但负载一高，一个真的
// 泄漏会静默地变绿——而那正是它要抓的东西。现在改成：再做**第三次**握手顶掉
// victim2，等 victim2 真的被关（**肯定**断言，等不到就 Fatal），然后不带任何
// sleep 地查 victim。
//
// 那次等待同时把原来那条独立的"前提测试"折了进来（旧的
// TestTheEvictionProbeUsedByTheLeakTestActuallyFires）：它证明的是"顶号确实会调
// Close"，而这里等 victim2 等到了，就地把同一件事证完了。前提和结论在同一个
// 函数里，跨测试依赖也就没有了——独立的前提测试是会被人单独删掉的，删掉之后
// 这里恒真且没有任何东西会响。
func TestAFailedReadyWriteDoesNotLeakTheSession(t *testing.T) {
	pub, priv := testKeys(t)
	r := router.New()
	cfg := Config{PublicKey: pub, MaxRX: 32}
	before := r.SessionCount()

	victim := &stubConn{}
	if _, err := handshake(helloStream(t, priv, "1000", true), victim, cfg, r, func([]byte) {}); err == nil {
		t.Fatal("handshake must fail when READY cannot be written")
	}
	if got := r.SessionCount(); got != before {
		t.Fatalf("sessions = %d, want %d — the session is registered before READY is written, so the failing path has to take it back out; the ghost keeps the cid and stays on the fan-out index", got, before)
	}

	// 同一个 CID 再登录两次。第二次要是找到了幽灵，它顶掉的是 victim；
	// 第三次顶掉的一定是 victim2。
	victim2 := &stubConn{}
	if _, err := handshake(helloStream(t, priv, "1000", false), victim2, cfg, r, func([]byte) {}); err != nil {
		t.Fatalf("the second handshake failed: %v", err)
	}
	if _, err := handshake(helloStream(t, priv, "1000", false), &stubConn{}, cfg, r, func([]byte) {}); err != nil {
		t.Fatalf("the third handshake failed: %v", err)
	}

	// 前提，肯定式：顶号真的会把被顶方关掉。等不到就说明这条探针根本不工作，
	// 下面那句"victim 没被关"也就什么都不证明——那时要红的是前提，不是结论。
	if !waitClosed(victim2, 3*time.Second) {
		t.Fatal("premise failed: a later login on the same cid did not close the session it displaced at all, so the assertion below proves nothing")
	}
	if victim.closed.Load() {
		t.Fatal("the failed handshake left its session in the router: the next login on the same cid found one to evict")
	}
}

// TestTheEvictionCloseDoesNotBlockTheNewHandshake 钉住顶号的断连是异步的。
//
// quic-go 的 CloseWithError 末尾是 `<-s.ctx.Done()`：它一直等到被关的那条连接的
// 主循环真的退出。而这个回调是 Router.Add 在**新会话的握手路径上**同步调用的，
// 所以直接调就是让新登录的人排队等旧连接拆完，而这条路径上没有任何超时救得了他。
//
// 上一轮我写过一条计时断言，去掉 `go` 之后本机上旧连接拆得太快（毫秒级）不红，
// 就把它删了。这里换成真的把断连按住：stubConn.block 让 CloseWithError 卡住，
// 于是快慢不再取决于本机有多快。
func TestTheEvictionCloseDoesNotBlockTheNewHandshake(t *testing.T) {
	pub, priv := testKeys(t)
	r := router.New()
	cfg := Config{PublicKey: pub, MaxRX: 32}

	release := make(chan struct{})
	defer close(release) // 收尾时放掉那个卡住的 goroutine

	victim := &stubConn{block: release}
	if _, err := handshake(helloStream(t, priv, "1000", false), victim, cfg, r, func([]byte) {}); err != nil {
		t.Fatalf("first handshake: %v", err)
	}

	done := make(chan error, 1)
	go func() {
		_, err := handshake(helloStream(t, priv, "1000", false), &stubConn{}, cfg, r, func([]byte) {})
		done <- err
	}()

	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("second handshake: %v", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("the second handshake is still waiting on the evicted connection's teardown; the eviction Close callback is running synchronously on the new session's handshake path")
	}
}
