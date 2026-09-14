package transport

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/tls"
	"encoding/binary"
	"errors"
	"fmt"
	"runtime"
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/auth"
	"github.com/JianyueLab-Org/can-voice/server/internal/control"
	"github.com/JianyueLab-Org/can-voice/server/internal/router"
	"github.com/quic-go/quic-go"
)

// 本文件钉的是一条连接的**两端**：关停时它怎么走，以及它不肯走的时候怎么被赶走。

// TestShuttingDownTellsEveryConnectedClientItMayReconnect 钉住关停时码 0 真的
// 送得出去。
//
// **重启部署是唯一会产生 CloseNormal 的路径**，所以它送不出去就等于这个码在生产上
// 从不存在——而客户端那套"码 0 可以重连、其它码别重连"的规则整个建立在它之上。
//
// 原来这里是 `defer ln.Close()`，accept 一返回就先关监听器，而 quic-go 的
// Listener.Close() 走 Transport.closeServer()：关掉 UDP socket，**从不遍历
// handlerMap**。于是每个客户端挂到自己的空闲超时，然后按"断网"处理。
//
// 断言必须是"拿到了一个码为 0 的 ApplicationError"，而不是"连接断了"：不修的话
// 连接最终**也会**断（空闲超时），只是要等几十秒、而且带的是超时错误。
func TestShuttingDownTellsEveryConnectedClientItMayReconnect(t *testing.T) {
	pub, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatalf("GenerateKey: %v", err)
	}
	cert, err := SelfSignedCert()
	if err != nil {
		t.Fatalf("SelfSignedCert: %v", err)
	}
	cfg := Config{
		Addr:      "127.0.0.1:0",
		TLS:       &tls.Config{Certificates: []tls.Certificate{cert}},
		PublicKey: pub,
		MaxRX:     32,
	}
	ln, err := listen(cfg)
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	stopped := make(chan error, 1)
	go func() { stopped <- serve(ctx, ln, cfg, router.New()) }()

	c := connect(t, ln.Addr().String(), "1000", 8, priv)
	// 前提：这条连接此刻是活的。不证的话，下面"它被关了"可能只是因为它从来
	// 就没连上。
	if pong := ping(t, c); pong.T != 42 {
		t.Fatalf("PONG.T = %d, want 42 — the connection was not healthy before the shutdown", pong.T)
	}

	cancel()

	select {
	case <-c.conn.Context().Done():
	case <-time.After(5 * time.Second):
		t.Fatal("a connected client was never told the server was shutting down; it will sit there until its own idle timeout and then treat a deploy as a network failure")
	}
	var appErr *quic.ApplicationError
	if !errors.As(context.Cause(c.conn.Context()), &appErr) {
		t.Fatalf("the connection ended with %v, want a *quic.ApplicationError — closing the listener alone never sends CONNECTION_CLOSE, so the client cannot tell a restart from a dead link", context.Cause(c.conn.Context()))
	}
	if appErr.ErrorCode != CloseNormal {
		t.Fatalf("close code = %d, want CloseNormal (%d) — that code is the client's only signal that reconnecting is the right answer", appErr.ErrorCode, CloseNormal)
	}

	select {
	case err := <-stopped:
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("serve returned %v, want context.Canceled", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("serve did not return after its context was cancelled")
	}
}

// TestShutdownStillClosesTheListener 是上一条的另一半：关停必须**真的**关掉
// 监听器，不能为了先送关闭码就把它落下。
//
// 落下的话，一个"已经退出"的进程还占着 UDP 端口——systemd 重启时新进程会以
// address already in use 起不来，而上一条测试完全看不见这件事。
func TestShutdownStillClosesTheListener(t *testing.T) {
	cert, err := SelfSignedCert()
	if err != nil {
		t.Fatalf("SelfSignedCert: %v", err)
	}
	pub, _, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatalf("GenerateKey: %v", err)
	}
	cfg := Config{
		Addr:      "127.0.0.1:0",
		TLS:       &tls.Config{Certificates: []tls.Certificate{cert}},
		PublicKey: pub,
		MaxRX:     32,
	}
	ln, err := listen(cfg)
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if err := serve(ctx, ln, cfg, router.New()); !errors.Is(err, context.Canceled) {
		t.Fatalf("serve returned %v, want context.Canceled", err)
	}
	// 关过的监听器再 Accept 必须立刻失败。
	actx, acancel := context.WithTimeout(context.Background(), time.Second)
	defer acancel()
	if _, err := ln.Accept(actx); err == nil {
		t.Fatal("the listener is still accepting after serve returned; the process would keep the UDP port and the next start would fail with address already in use")
	}
}

// TestARefusedHandshakeDoesNotLeakAGoroutine 钉住被拒路径上的 out.stop()。
//
// 每条连接一建立就起一个排空 goroutine（newOutbound 里那个 `go o.run()`），
// 而它是在**握手之前**建的——收尾要用到它。握手被拒时那条路径必须自己停掉它，
// 否则每一次被拒的握手漏一个 goroutine，**一张坏 token 就够了**，不需要通过
// 鉴权。实测：50 次被拒握手，goroutine 增量恰好 50。
//
// 这条测试直接调 handleConn 而不走真 QUIC：真连接每条都会带进来一把 quic-go
// 自己的 goroutine，它们的退出时机跟我们无关，数出来的差值会被淹掉。假连接下
// 这个数是确定的。
func TestARefusedHandshakeDoesNotLeakAGoroutine(t *testing.T) {
	const rounds = 50

	restore := byeGrace.Set(time.Millisecond)
	defer byeGrace.Set(restore)

	pub, _ := testKeys(t)
	_, otherPriv := testKeys(t) // 用别的私钥签，所以必然验不过
	cfg := Config{PublicKey: pub, MaxRX: 32}
	r := router.New()

	refuse := func() {
		tok, err := auth.Sign(otherPriv, auth.Claims{
			CID: "1000", Rating: 5, MaxTX: 8, Exp: time.Now().Add(time.Minute).Unix(),
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
		st := &memStream{in: bytes.NewReader(framed.Bytes())}
		handleConn(context.Background(), &stubConn{stream: st}, cfg, r)
	}

	// 先跑一轮，把一次性的东西（包级初始化、第一次分配）挪到基线之外。
	refuse()
	base := settledGoroutines(t)

	for i := 0; i < rounds; i++ {
		refuse()
	}
	// 前提：这些握手确实全都被拒了，没有一条留下会话。否则"没漏 goroutine"
	// 可能只是因为根本没走到那条路径上。
	if got := r.SessionCount(); got != 0 {
		t.Fatalf("sessions = %d, want 0 — these handshakes were supposed to be refused", got)
	}

	after := settledGoroutines(t)
	if after > base {
		t.Fatalf("%d refused handshakes left %d extra goroutines behind (%d → %d); the outbound drain goroutine is started before the handshake and a refused handshake must stop it — one bad token is enough to do this, no authentication required",
			rounds, after-base, base, after)
	}
}

// settledGoroutines 等 goroutine 数稳定下来再读它。
//
// 直接读会把还在退出路上的 goroutine 算进去——那不是泄漏，只是还没被调度到。
// 连续几次读到同一个数才算稳定；一直不稳定就取最后一次，让断言自己去判。
func settledGoroutines(t *testing.T) int {
	t.Helper()
	const stableRounds = 5
	last, stable := -1, 0
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		n := runtime.NumGoroutine()
		if n == last {
			if stable++; stable >= stableRounds {
				return n
			}
		} else {
			last, stable = n, 0
		}
		time.Sleep(5 * time.Millisecond)
	}
	return last
}

// TestAPeerThatStartsAControlFrameAndStopsIsClosed 钉住读侧的截止时间。
//
// 手法：写一个声称 64 KiB 的长度前缀，然后就不发了（真实攻击里是每 30 秒发一个
// 字节）。control.ReadFrame 永远停在 io.ReadFull 里，这条会话和它的三个
// goroutine 被永久占住，而 **QUIC 的空闲计时器不会开火**——包确实在到达。
// 写侧的同一个形状 Task 9C 已经堵上了，读侧一直没有。
func TestAPeerThatStartsAControlFrameAndStopsIsClosed(t *testing.T) {
	restore := controlReadTimeout.Set(300 * time.Millisecond)
	defer controlReadTimeout.Set(restore)

	addr, priv, _ := testServer(t)
	c := connect(t, addr, "1000", 8, priv)

	// 一个合法的长度前缀（65535 ≤ control.MaxFrame），后面只给一个字节。
	var hdr [4]byte
	binary.BigEndian.PutUint32(hdr[:], control.MaxFrame-1)
	if _, err := c.st.Write(hdr[:]); err != nil {
		t.Fatalf("Write length prefix: %v", err)
	}
	if _, err := c.st.Write([]byte{'{'}); err != nil {
		t.Fatalf("Write one byte of payload: %v", err)
	}

	ctx := c.conn.Context()
	select {
	case <-ctx.Done():
	case <-time.After(10 * time.Second):
		t.Fatal("a peer that promised a 64 KiB frame and then stopped sending it kept a session and three goroutines forever; the idle timer never fires because packets really are arriving")
	}
	var appErr *quic.ApplicationError
	if !errors.As(context.Cause(ctx), &appErr) {
		t.Fatalf("connection closed with %v, want a *quic.ApplicationError", context.Cause(ctx))
	}
	if appErr.ErrorCode != CloseProtocolViolation {
		t.Fatalf("close code = %d, want CloseProtocolViolation (%d) — CloseNormal would tell the client to reconnect and replay the same bug", appErr.ErrorCode, CloseProtocolViolation)
	}
	if appErr.ErrorMessage != ReasonControlReadStalled {
		t.Fatalf("reason = %q, want %q", appErr.ErrorMessage, ReasonControlReadStalled)
	}
}

// TestASilentControlStreamIsNotClosedByTheReadDeadline 是上一条的前提证明，
// 而且是它最容易被写坏的那一半。
//
// "给读加个截止时间"最直白的写法——整条流一个读截止时间——会通过上一条测试，
// 然后在生产上把每一个**几分钟不说话的管制员**掐掉。分界线不是"你有没有在说话"，
// 是"你已经承诺了一帧没有"。
//
// 必须用**同一个**被调小的时限：换成一个宽松的值，这条测试就什么都不证明了。
func TestASilentControlStreamIsNotClosedByTheReadDeadline(t *testing.T) {
	restore := controlReadTimeout.Set(200 * time.Millisecond)
	defer controlReadTimeout.Set(restore)

	addr, priv, _ := testServer(t)
	c := connect(t, addr, "1000", 8, priv)

	// 什么都不发，睡过时限的三倍。
	time.Sleep(600 * time.Millisecond)

	select {
	case <-c.conn.Context().Done():
		t.Fatalf("the read deadline fired on a stream that had not started a frame at all: %v — in production that closes every controller who goes quiet for a few minutes", context.Cause(c.conn.Context()))
	default:
	}
	if pong := ping(t, c); pong.T != 42 {
		t.Fatalf("PONG.T = %d, want 42 — the session must survive an ordinary silence", pong.T)
	}

	// 而且一帧读完之后截止时间要被撤掉：不撤的话，下一次静默会继承一个已经
	// 过去的时刻，每条会话在收到第一帧之后不久自己死掉。上面那次 PING 已经
	// 让服务端读完了一整帧，所以再睡一次就能分辨。
	time.Sleep(600 * time.Millisecond)
	if pong := ping(t, c); pong.T != 42 {
		t.Fatalf("PONG.T = %d, want 42 — the read deadline was not cleared after a complete frame, so every session dies shortly after its first message", pong.T)
	}
}

// TestAnUndeliverableSubAckClosesLoudly 钉住「SUBACK 发不出去」那条防线。
//
// 形状：一份 SUB **已经生效了**，而它的 SUBACK 大到发不出去。老代码是
// `if out, err := control.Encode(&ack); err == nil { … }`，写失败一声不吭地
// 落到外层那条 defer 的 CloseNormal——实测是 `Application error 0x0`、reason 为空、
// 日志只有一行 `session closed dropped=0`。而码 0 的意思是"你可以重连"，客户端
// 于是重连、重放同一份 SUB，再一次静默失败：两端都没有一个字的死循环。
//
// 声明本身有了上界（router 的 declarationLimit）之后这条路应当不可达，所以测试
// 直接构造一条 MaxRX 大到
// 离谱的会话——那是 router 允许、而传输层自己永远不会造出来的组合。一条防线
// 要在**它自己那一层**被钉住，否则"现在够不着"这个结论会被下一次改动悄悄推翻，
// 而没有任何东西会红。
func TestAnUndeliverableSubAckClosesLoudly(t *testing.T) {
	r := router.New()
	// 塞满一条 64 KiB 的 SUB：小数字占的字节少，所以 0,1,2,… 是能装进最多频率
	// 的形状。数量**量出来**而不是写死——写死一个数字会在 JSON 形状变一点
	// （比如 nil 切片编成 null 而不是 []）的时候悄悄失去意义。
	// 回来的 SubAck 带 RX 和 TX 两份，所以必然超过帧上限。
	var tx []uint32
	var b []byte
	for n := 13000; n > 0; n-- {
		freqs := make([]uint32, 0, n)
		for i := 0; i < n; i++ {
			freqs = append(freqs, uint32(i))
		}
		encoded, err := control.Encode(&control.Sub{TX: freqs})
		if err != nil {
			t.Fatalf("Encode SUB: %v", err)
		}
		if len(encoded) <= control.MaxFrame {
			tx, b = freqs, encoded
			break
		}
	}
	if b == nil {
		t.Fatal("could not build a SUB that fits the frame limit")
	}
	t.Logf("the SUB carries %d TX frequencies in %d bytes (limit %d)", len(tx), len(b), control.MaxFrame)

	s := r.Add(router.SessionOpts{
		CID: "1000", MaxTX: len(tx), MaxRX: len(tx), Send: func([]byte) {},
	})

	var framed bytes.Buffer
	if err := control.WriteFrame(&framed, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}

	st := &memStream{in: bytes.NewReader(framed.Bytes())}
	if err := readControl(st, r, s); !errors.Is(err, errAckUndeliverable) {
		t.Fatalf("readControl returned %v, want errAckUndeliverable — a SUB that took effect and got no ACK must end the session loudly, not drift into CloseNormal and invite the client to replay it forever", err)
	}
	// 前提：这份 SUB 确实生效了。没生效的话上面那句说的就是另一件事。
	if !r.MayTransmit(s.ID, 0) {
		t.Fatal("premise failed: the subscription did not take effect at all, so there was no state mismatch to report")
	}

	// 对照：同样这条路径上，一份**发得出去**的 SUBACK 必须以 nil 收场。
	// 没有这一半的话，一个把每条 SUB 都判成发不出去的实现照样绿。
	small, err := control.Encode(&control.Sub{TX: []uint32{118000}})
	if err != nil {
		t.Fatalf("Encode SUB: %v", err)
	}
	framed.Reset()
	if err := control.WriteFrame(&framed, small); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	st = &memStream{in: bytes.NewReader(framed.Bytes())}
	if err := readControl(st, r, s); err != nil {
		t.Fatalf("readControl returned %v for an ordinary SUB, want nil", err)
	}
}

// TestTheControlStreamDeathsMapToTheCodesTheProtocolNames 钉住那张翻译表。
//
// 它单独存在，是因为走到这三种死因的路各有各的难走：一种要造一个会卡住流控的
// 对端，一种要一帧永远发不完，第三种在声明有了上界之后根本不可达。而这张表本身是
// **协议**——码和原因串是客户端写死在自己代码里的字面值。
func TestTheControlStreamDeathsMapToTheCodesTheProtocolNames(t *testing.T) {
	for _, tc := range []struct {
		name   string
		err    error
		code   quic.ApplicationErrorCode
		reason string
		close  bool
	}{
		{"a clean end", nil, 0, "", false},
		{"the write stalled", errControlWriteStalled, CloseProtocolViolation, "control_write_stalled", true},
		{"the read stalled", errControlReadStalled, CloseProtocolViolation, "control_read_stalled", true},
		{"the ack could not be sent", errAckUndeliverable, CloseProtocolViolation, "ack_undeliverable", true},
		// 别的写错误走正常收尾：那时候"你可以重连"是对的答案。
		{"an ordinary write error", errors.New("stream closed"), 0, "", false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			code, reason, closes := closeAfterControl(tc.err)
			if closes != tc.close {
				t.Fatalf("closeAfterControl(%v) closes = %v, want %v", tc.err, closes, tc.close)
			}
			if code != tc.code || reason != tc.reason {
				t.Fatalf("closeAfterControl(%v) = (%d, %q), want (%d, %q) — these are literal values a client matches on",
					tc.err, code, reason, tc.code, tc.reason)
			}
		})
	}
}

// fakeRemover 在被 Remove 的那一刻回头问队列"你停了没有"。
//
// 这就是让拆除顺序变成可断言的事实的全部机关：两行代码的相对位置本身没法断言，
// 但"Remove 被调到时队列是否已经停了"可以。
type fakeRemover struct {
	out          *outbound
	called       bool
	stoppedFirst bool
}

func (f *fakeRemover) Remove(router.SessionID) {
	f.called = true
	f.stoppedFirst = f.out.stopped()
}

// TestTheSessionIsUnregisteredBeforeItsQueueStops 钉住收尾的顺序。
//
// 这条顺序在 conn.go 里带着一段注释、预言了一个具体症状，而在此之前把两行互换
// **整套测试照绿**——正是"有文档、无钉子"的那一类。
//
// 症状：反过来的话，在摘除之前最后被扇进来的那几帧会留在一条已经没人排空的
// 队列里，而它们很可能正是带 FlagLast 的尾帧。那一位是接收端用来熄灭 RX 指示灯
// 的（wire/header.go 写明它存在就是为了取代 can-audio 那个"松开 PTT 后指示灯多
// 亮半秒"的超时循环），丢了对端的灯就一直亮着，没有任何东西会来纠正。
func TestTheSessionIsUnregisteredBeforeItsQueueStops(t *testing.T) {
	o := newOutbound(func([]byte) error { return nil })
	rec := &fakeRemover{out: o}

	closeSession(rec, router.SessionID(1), o)

	// 前提：Remove 真的被调到了。少了这一句，"停在它后面"在一个根本不调
	// Remove 的实现上平凡为真。
	if !rec.called {
		t.Fatal("premise failed: closeSession never removed the session at all, so the ordering assertion below means nothing")
	}
	if rec.stoppedFirst {
		t.Fatal("the outbound queue was stopped before the session was unregistered: the frames fanned in during that window sit in a queue nobody drains, and the ones most likely to be there are the FlagLast tail frames — the peer's RX light then stays lit forever, with nothing to correct it")
	}
	// 另一半：队列**确实**停了。顺序对而根本没停，会每条会话漏一个 goroutine。
	if !o.stopped() {
		t.Fatal("closeSession returned without stopping the outbound queue; every session would leak its drain goroutine")
	}
}

// TestAnEstablishedSessionDoesNotLeakItsDrainGoroutine 是被拒那条
// （TestARefusedHandshakeDoesNotLeakAGoroutine）的另一半。
//
// 那一条只走握手失败的路径，所以整段 `defer { closeSession(...) }` 可以被删光
// 而它照样绿。这一条走**握手成功**的那条路：每条连接一建立就起一个排空
// goroutine，正常收尾必须停掉它，否则每一个登录过的人都留下一个。
//
// 和那一条一样直接调 handleConn 而不走真 QUIC：真连接每条都会带进来一把
// quic-go 自己的 goroutine，退出时机跟我们无关，差值会被淹掉。
func TestAnEstablishedSessionDoesNotLeakItsDrainGoroutine(t *testing.T) {
	const rounds = 50

	pub, priv := testKeys(t)
	cfg := Config{PublicKey: pub, MaxRX: 32}
	r := router.New()

	// readDatagrams 跟着这个 ctx 走，所以测完要取消它——不取消的话它自己就是
	// 一堆不会退出的 goroutine，把要数的那个差值淹掉。
	ctx, cancel := context.WithCancel(context.Background())

	// cid 每轮都不同：同一个 cid 会走顶号路径，而顶号会再起一个 goroutine 去
	// 断旧连接，那是另一件事的噪音。
	//
	// 每一轮都验一次"服务端回的是 READY"。前提不能靠事后数会话数——handleConn
	// 返回时它自己的 defer 已经把会话摘掉了，所以那个数永远是 0，跟握手成没成功
	// 无关。而握手要是**全被拒**，走的就是另一条路径，这条测试就什么都没测。
	serve := func(n int) {
		st := helloStream(t, priv, fmt.Sprintf("10%02d", n), false)
		handleConn(ctx, &stubConn{stream: st}, cfg, r)
		raw, err := control.ReadFrame(&st.out)
		if err != nil {
			t.Fatalf("round %d: the server wrote no reply at all: %v", n, err)
		}
		m, err := control.Decode(raw)
		if err != nil {
			t.Fatalf("round %d: Decode: %v", n, err)
		}
		if _, ok := m.(*control.Ready); !ok {
			t.Fatalf("round %d: the handshake got %T, want *control.Ready — a refused handshake exercises a different teardown path, which already has its own test", n, m)
		}
	}

	serve(0)
	base := settledGoroutines(t)

	for i := 1; i <= rounds; i++ {
		serve(i)
	}

	cancel()
	after := settledGoroutines(t)
	if after > base {
		t.Fatalf("%d established-then-closed sessions left %d extra goroutines behind (%d → %d); the drain goroutine is started per connection and the teardown must stop it",
			rounds, after-base, base, after)
	}
}

// TestTheListenerKeepsSilentSessionsAlive 钉住 quic.Config 的三个值。
//
// 它们此前一个都钉不住，因为整个 config 是内联在 ListenAddr 的实参里的：
// KeepAlivePeriod 改成 0、MaxIdleTimeout 改成任何别的数、EnableDatagrams 去掉，
// 整套测试照绿——而三者各自都有一条能在生产上咬人的后果。
//
// KeepAlivePeriod=0 是里面最阴的一个：零值的意思是"不发保活"，而这条连接上
// 安静几分钟完全正常（一个只监听、不讲话的管制员）。没有保活，空闲计时器 60 秒
// 后开火把他断开；重连之后一切正常，于是表现是"每隔一分钟掉一次线"，日志里只有
// 一条普通的超时。listen() 那段注释（"管制员可能几分钟不说话，但 QUIC 的保活会
// 撑住连接"）论证的恰恰是这一行。
func TestTheListenerKeepsSilentSessionsAlive(t *testing.T) {
	c := quicConfig()
	if !c.EnableDatagrams {
		t.Fatal("EnableDatagrams is off: the handshake and the control plane would still work, while every SendDatagram fails with \"datagram support disabled\" — everyone lit up on the board and nobody able to hear anyone")
	}
	if c.KeepAlivePeriod <= 0 {
		t.Fatal("KeepAlivePeriod is 0, which means \"send no keep-alives\": a controller who is only listening gets dropped by the idle timer every minute, reconnects fine, and the log says nothing but \"timeout\"")
	}
	if c.MaxIdleTimeout != 60*time.Second {
		t.Fatalf("MaxIdleTimeout = %v, want 60s — it has to outlast any ordinary silence on a voice channel", c.MaxIdleTimeout)
	}
	if c.KeepAlivePeriod*2 >= c.MaxIdleTimeout {
		t.Fatalf("KeepAlivePeriod (%v) leaves no room for a lost PING inside MaxIdleTimeout (%v); it must be comfortably under half", c.KeepAlivePeriod, c.MaxIdleTimeout)
	}
}

// TestListenRefusesToStartWithoutACertificate 钉住 listen() 的那道 nil 守卫。
//
// 没有它，下一行的 cfg.TLS.Clone() 返回 nil，给 nil 写 NextProtos 直接 panic，
// 而 panic 的堆栈指向 crypto/tls——运维看到的是一个崩溃，不是"配置里少了证书"。
func TestListenRefusesToStartWithoutACertificate(t *testing.T) {
	defer func() {
		if v := recover(); v != nil {
			t.Fatalf("listen panicked instead of returning an error: %v — the stack points at crypto/tls, not at the missing certificate", v)
		}
	}()
	if _, err := listen(Config{Addr: "127.0.0.1:0"}); err == nil {
		t.Fatal("listen accepted a Config with no TLS at all")
	}
}
