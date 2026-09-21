package transport

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/tls"
	"errors"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/auth"
	"github.com/JianyueLab-Org/can-voice/server/internal/control"
	"github.com/JianyueLab-Org/can-voice/server/internal/router"
	"github.com/JianyueLab-Org/can-voice/server/internal/wire"
	"github.com/quic-go/quic-go"
)

func testServer(t *testing.T) (addr string, priv ed25519.PrivateKey, r *router.Router) {
	t.Helper()
	pub, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatalf("GenerateKey: %v", err)
	}
	cert, err := SelfSignedCert()
	if err != nil {
		t.Fatalf("SelfSignedCert: %v", err)
	}
	r = router.New()
	// 一个 cfg，两处都用。原文给 listen 和 accept 传了两个不同的 Config
	// （后者少了 TLS 和 Addr），碰巧能跑——因为 accept 用不到那两项——
	// 但下一个人加一个 accept 真的要读的字段时就会踩空。
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
	t.Cleanup(func() { cancel(); ln.Close() })
	go accept(ctx, ln, cfg, r, newConnSet())
	return ln.Addr().String(), priv, r
}

func dial(t *testing.T, addr string) quic.Connection {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	t.Cleanup(cancel)
	conn, err := quic.DialAddr(ctx, addr, &tls.Config{
		InsecureSkipVerify: true,
		NextProtos:         []string{ALPN},
	}, &quic.Config{EnableDatagrams: true})
	if err != nil {
		t.Fatalf("DialAddr: %v", err)
	}
	t.Cleanup(func() { conn.CloseWithError(0, "") })
	return conn
}

func hello(t *testing.T, conn quic.Connection, token string) (quic.Stream, any) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	st, err := conn.OpenStreamSync(ctx)
	if err != nil {
		t.Fatalf("OpenStreamSync: %v", err)
	}
	b, _ := control.Encode(&control.Hello{Token: token, Client: "test/1", Proto: 1})
	if err := control.WriteFrame(st, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	resp, err := control.ReadFrame(st)
	if err != nil {
		t.Fatalf("ReadFrame: %v", err)
	}
	m, err := control.Decode(resp)
	if err != nil {
		t.Fatalf("Decode: %v", err)
	}
	return st, m
}

func TestHelloWithAValidTokenGetsReady(t *testing.T) {
	addr, priv, r := testServer(t)
	tok, err := auth.Sign(priv, auth.Claims{CID: "1000", Rating: 5, MaxTX: 8, Exp: time.Now().Add(time.Minute).Unix()})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	_, m := hello(t, dial(t, addr), tok)

	ready, ok := m.(*control.Ready)
	if !ok {
		t.Fatalf("server replied %T, want *control.Ready", m)
	}
	if ready.Session == 0 {
		t.Fatal("READY must carry a session id; it becomes the speaker field in every packet")
	}
	if ready.MaxTX != 8 {
		t.Fatalf("READY.MaxTX = %d, want the token's 8", ready.MaxTX)
	}
	if ready.Server != ServerVersion {
		t.Fatalf("READY.Server = %q, want %q", ready.Server, ServerVersion)
	}
	if _, ok := r.Get(router.SessionID(ready.Session)); !ok {
		t.Fatal("the session must be registered with the router")
	}
}

// TestHelloWithABadTokenGetsByeAndNoSession 钉住被拒的握手不留下会话。
//
// 断言的是**会话数没有增加**，不是"某个具体 id 不存在"。原文查的是
// r.Get(SessionID(1))，而 newSessionID 由进程全局的 nextID 驱动、从不重置：
// 整包一起跑的时候 1 号早就被别的测试用掉了，所以那句话恒真。实测过——
// 在被拒路径上**故意**登记一条会话，整包跑全绿，
// 只有 `-run` 单跑才红。而 CI 跑的正是整包。
func TestHelloWithABadTokenGetsByeAndNoSession(t *testing.T) {
	addr, priv, r := testServer(t)
	_, otherPriv, _ := ed25519.GenerateKey(rand.Reader)
	tok, _ := auth.Sign(otherPriv, auth.Claims{CID: "1000", Exp: time.Now().Add(time.Minute).Unix()})

	before := r.SessionCount()
	_, m := hello(t, dial(t, addr), tok)
	bye, ok := m.(*control.Bye)
	if !ok {
		t.Fatalf("server replied %T, want *control.Bye", m)
	}
	if bye.Reason == "" {
		t.Fatal("BYE must say why")
	}
	if got := r.SessionCount(); got != before {
		t.Fatalf("sessions = %d, want %d — a rejected HELLO must not leave a session behind", got, before)
	}

	// 上面那一条是一句**缺席断言**，而缺席断言要自己证明前提：这个 router 上
	// 一次成功的握手确实会让会话数 +1。不证的话，"握手根本没接上 router"
	// 也会让它绿。
	good, err := auth.Sign(priv, auth.Claims{CID: "1001", Rating: 5, MaxTX: 1, Exp: time.Now().Add(time.Minute).Unix()})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	_, m = hello(t, dial(t, addr), good)
	ready, ok := m.(*control.Ready)
	if !ok {
		t.Fatalf("premise check: a good token got %T, want *control.Ready", m)
	}
	if _, ok := r.Get(router.SessionID(ready.Session)); !ok {
		t.Fatal("premise check failed: r.Get returns false even for a session that was just registered, so the assertion above proved nothing")
	}
	if got := r.SessionCount(); got != before+1 {
		t.Fatalf("premise check failed: sessions = %d after one successful handshake, want %d — the count does not move, so the assertion above proved nothing", got, before+1)
	}
}

func TestSubGetsSubAckAndTakesEffect(t *testing.T) {
	addr, priv, r := testServer(t)
	tok, _ := auth.Sign(priv, auth.Claims{CID: "1000", Rating: 5, MaxTX: 8, Exp: time.Now().Add(time.Minute).Unix()})
	st, m := hello(t, dial(t, addr), tok)
	ready := m.(*control.Ready)

	b, _ := control.Encode(&control.Sub{RX: []uint32{118000, 121800}, TX: []uint32{121800}})
	if err := control.WriteFrame(st, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	resp, err := control.ReadFrame(st)
	if err != nil {
		t.Fatalf("ReadFrame: %v", err)
	}
	dm, err := control.Decode(resp)
	if err != nil {
		t.Fatalf("Decode: %v", err)
	}
	if _, ok := dm.(*control.SubAck); !ok {
		t.Fatalf("server replied %T, want *control.SubAck", dm)
	}
	if len(r.Listeners(118000)) != 1 {
		t.Fatal("SUB did not take effect in the router")
	}
	if !r.MayTransmit(router.SessionID(ready.Session), 121800) {
		t.Fatal("SUB did not register the TX frequency")
	}
}

func TestAFirstMessageOtherThanHelloIsRefused(t *testing.T) {
	addr, priv, r := testServer(t)
	conn := dial(t, addr)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	st, err := conn.OpenStreamSync(ctx)
	if err != nil {
		t.Fatalf("OpenStreamSync: %v", err)
	}
	b, _ := control.Encode(&control.Sub{RX: []uint32{118000}})
	if err := control.WriteFrame(st, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	resp, err := control.ReadFrame(st)
	if err != nil {
		t.Fatalf("ReadFrame: %v", err)
	}
	m, _ := control.Decode(resp)
	bye, ok := m.(*control.Bye)
	if !ok {
		t.Fatalf("server replied %T, want *control.Bye — nothing may precede HELLO", m)
	}
	// 只断言"是 BYE"挡不住任何东西：把这道闸换成"当作一条空 HELLO 处理"，
	// 空 token 一样验不过，回的还是 BYE，测试照样绿（实测过）。原因必须可分辨——
	// "refused" 说的是"这条消息就不该出现在这里"，"token_invalid" 说的是
	// "我们真的去验了一张 token"，而后者意味着这道闸已经没了。
	if bye.Reason != "refused" {
		t.Fatalf("Reason = %q, want \"refused\" — a non-HELLO first message must be turned away before any token is looked at", bye.Reason)
	}
	// 而且它不能改动任何状态：未鉴权的连接一个字节的服务端状态都不该动得了。
	if len(r.Listeners(118000)) != 0 {
		t.Fatal("a SUB that arrived before HELLO took effect in the router")
	}

	// 缺席断言的前提证明：同一个 router 上，一条鉴权过的 SUB 确实会让
	// Listeners(118000) 变成 1。
	tok, err := auth.Sign(priv, auth.Claims{CID: "1000", Rating: 5, MaxTX: 1, Exp: time.Now().Add(time.Minute).Unix()})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	good, m2 := hello(t, dial(t, addr), tok)
	if _, ok := m2.(*control.Ready); !ok {
		t.Fatalf("premise check: a good token got %T, want *control.Ready", m2)
	}
	if err := control.WriteFrame(good, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	if _, err := control.ReadFrame(good); err != nil {
		t.Fatalf("ReadFrame (SUBACK): %v", err)
	}
	if len(r.Listeners(118000)) != 1 {
		t.Fatal("premise check failed: even an authenticated SUB does not show up in Listeners(118000), so the assertion above proved nothing")
	}
}

// TestAConnectionThatSendsNothingIsDroppedAtTheHandshakeDeadline 钉住 slowloris 那道闸。
//
// 这是公网 UDP 端口。一个建好 QUIC 连接、然后一个字节都不发的对端，没有超时的话
// 会永久占住一个 goroutine 和一条连接——不需要带宽，不需要有效凭据。
func TestAConnectionThatSendsNothingIsDroppedAtTheHandshakeDeadline(t *testing.T) {
	orig := handshakeTimeout.Set(200 * time.Millisecond)
	defer handshakeTimeout.Set(orig)

	addr, _, _ := testServer(t)
	conn := dial(t, addr)
	// 故意不开流、不发任何东西。

	// 判据是"服务端把连接关掉了"，而不是"客户端这边某个调用返回了错误"。
	// 原文断言的是 conn.AcceptStream(ctx) != nil，而那个 ctx 自己带 3 秒超时——
	// 服务端就算永远不关，它到点也会返回 context.DeadlineExceeded，测试照样绿。
	// 实测过：把 handshakeTimeout 那道闸整个拆掉，原来的断言一次都没红。
	select {
	case <-conn.Context().Done():
	case <-time.After(3 * time.Second):
		t.Fatal("the server kept a silent connection open past the handshake deadline; that is a slowloris on a public UDP port")
	}
}

// TestAStreamOpenedButNeverWrittenIsAlsoDropped 覆盖"开了流但一个字节都不写"。
//
// **它钉住的其实仍然是 AcceptStream 那道闸，不是读截止时间**，这一点必须写明白，
// 否则下一个人会以为读截止时间有人管。QUIC 的流是惰性的：OpenStreamSync 只在
// 本地占一个流号，对端真的发出字节之前服务端的 AcceptStream 不会返回——所以这条
// 连接从服务端看和"一言不发"没有区别。实测过：把 SetReadDeadline 整行删掉，
// 这条测试一次都没红；把 AcceptStream 的超时去掉，它才红。
// 读截止时间由 TestAHalfWrittenFirstFrameIsDroppedAtTheReadDeadline 钉住。
func TestAStreamOpenedButNeverWrittenIsAlsoDropped(t *testing.T) {
	orig := handshakeTimeout.Set(200 * time.Millisecond)
	defer handshakeTimeout.Set(orig)

	addr, _, _ := testServer(t)
	conn := dial(t, addr)
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	if _, err := conn.OpenStreamSync(ctx); err != nil {
		t.Fatalf("OpenStreamSync: %v", err)
	}
	// 开了流，但一个字节都不写。

	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		if conn.Context().Err() != nil {
			return // 连接被关掉了，正确
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("a stream that was opened but never written kept the connection alive past the handshake deadline; the AcceptStream timeout is missing")
}

// TestAHalfWrittenFirstFrameIsDroppedAtTheReadDeadline 是上面那条**真正**该测的东西。
//
// QUIC 的流是惰性的：客户端 OpenStreamSync 只是占了一个流号，服务端的 AcceptStream
// 在对端真的发出字节之前不会返回。所以上一条测试其实还是被 AcceptStream 的 ctx
// 超时挡下的，把 SetReadDeadline 整个删掉它照样绿——一条钉不住自己那个机制的钉子。
//
// 这里补上另一半：先写半个长度前缀，服务端的 AcceptStream 因此返回，
// handshake 进到 control.ReadFrame 并阻塞在那里。这时 ctx 已经帮不上忙
// （取消一个 ctx 不会打断一个已经进了 Read 的流），只有读截止时间能收场。
func TestAHalfWrittenFirstFrameIsDroppedAtTheReadDeadline(t *testing.T) {
	orig := handshakeTimeout.Set(200 * time.Millisecond)
	defer handshakeTimeout.Set(orig)

	addr, _, _ := testServer(t)
	conn := dial(t, addr)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	st, err := conn.OpenStreamSync(ctx)
	if err != nil {
		t.Fatalf("OpenStreamSync: %v", err)
	}
	// 4 字节长度前缀只写 2 个：服务端拿得到流，然后永远等不到剩下的。
	if _, err := st.Write([]byte{0x00, 0x00}); err != nil {
		t.Fatalf("Write: %v", err)
	}

	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if conn.Context().Err() != nil {
			return // 连接被关掉了，正确
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("a connection that sent half a length prefix and then stopped stayed open past the handshake deadline; the stream read deadline is missing")
}

// TestABadTokenGetsACoarseReasonNotTheInternalError 钉住不向未鉴权对端泄露内部错误。
func TestABadTokenGetsACoarseReasonNotTheInternalError(t *testing.T) {
	addr, priv, r := testServer(t)
	conn := dial(t, addr)
	_, m := hello(t, conn, "this.is-not-a-token")

	bye, ok := m.(*control.Bye)
	if !ok {
		t.Fatalf("got %T, want *control.Bye", m)
	}
	if bye.Reason != "token_invalid" {
		t.Fatalf("Reason = %q, want the stable coarse code \"token_invalid\" — the internal error text must not reach an unauthenticated peer", bye.Reason)
	}
	// 不许出现内部细节。
	for _, leak := range []string{"base64", "JSON", "signature", "length", "payload"} {
		if strings.Contains(strings.ToLower(bye.Reason), strings.ToLower(leak)) {
			t.Fatalf("Reason = %q leaks %q; that only helps someone forging a token", bye.Reason, leak)
		}
	}
	if len(r.Listeners(121800)) != 0 {
		t.Fatal("a refused handshake must not have registered a session")
	}

	// 缺席断言的前提证明：同一个 router 上，一次真的 HELLO+SUB 确实会让
	// Listeners(121800) 变成 1。不证的话，"扇出索引根本没接上"也会让上面绿。
	good, err := auth.Sign(priv, auth.Claims{CID: "1001", Rating: 5, MaxTX: 1, Exp: time.Now().Add(time.Minute).Unix()})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	st, m := hello(t, dial(t, addr), good)
	if _, ok := m.(*control.Ready); !ok {
		t.Fatalf("premise check: a good token got %T, want *control.Ready", m)
	}
	b, _ := control.Encode(&control.Sub{RX: []uint32{121800}})
	if err := control.WriteFrame(st, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	if _, err := control.ReadFrame(st); err != nil {
		t.Fatalf("ReadFrame (SUBACK): %v", err)
	}
	if len(r.Listeners(121800)) != 1 {
		t.Fatal("premise check failed: even a real subscription does not show up in Listeners(121800), so the assertion above proved nothing")
	}
}

// TestAnExpiredTokenIsDistinguishableFromAnInvalidOne 钉住恰好两级。
//
// 正当客户端需要区分"去换一张新 token 再试"和"别试了"：没有这个区别它只能
// 盲目重试，而重试会撞上限速。
func TestAnExpiredTokenIsDistinguishableFromAnInvalidOne(t *testing.T) {
	addr, priv, _ := testServer(t)
	tok, err := auth.Sign(priv, auth.Claims{
		CID: "1000", Rating: 5, MaxTX: 8,
		Exp: time.Now().Add(-time.Minute).Unix(),
	})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	conn := dial(t, addr)
	_, m := hello(t, conn, tok)
	bye, ok := m.(*control.Bye)
	if !ok {
		t.Fatalf("got %T, want *control.Bye", m)
	}
	if bye.Reason != "token_expired" {
		t.Fatalf("Reason = %q, want \"token_expired\" — a client that cannot tell this from an unusable token can only retry blindly, and retrying hits the rate limit", bye.Reason)
	}
	// 而且不能把服务端的时钟送出去。
	if strings.ContainsAny(bye.Reason, "0123456789") {
		t.Fatalf("Reason = %q carries a number; the detailed message names the server's clock and must stay in the log", bye.Reason)
	}
}

// TestARefusedHandshakeIsClosedWithTheRefusedCode 钉住被拒的连接带的是
// CloseHandshakeRefused 而不是 CloseNormal。
//
// 两者对客户端的意思相反：CloseNormal 说"可以重连"，CloseHandshakeRefused 说
// "先换一张 token"。漂成 0 的症状是客户端拿着一张废 token 不停重连，
// 然后在 can-api 那边把自己的 CID 限流掉。
func TestARefusedHandshakeIsClosedWithTheRefusedCode(t *testing.T) {
	addr, priv, _ := testServer(t)
	tok, err := auth.Sign(priv, auth.Claims{
		CID: "1000", Rating: 5, MaxTX: 8, Exp: time.Now().Add(-time.Minute).Unix(),
	})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	conn := dial(t, addr)
	if _, m := hello(t, conn, tok); func() bool { _, ok := m.(*control.Bye); return !ok }() {
		t.Fatal("a refused handshake must answer with BYE")
	}

	select {
	case <-conn.Context().Done():
	case <-time.After(5 * time.Second):
		t.Fatal("the refused connection was never closed")
	}
	var appErr *quic.ApplicationError
	if !errors.As(context.Cause(conn.Context()), &appErr) {
		t.Fatalf("refused connection closed with %v, want a *quic.ApplicationError", context.Cause(conn.Context()))
	}
	if appErr.ErrorCode != CloseHandshakeRefused {
		t.Fatalf("close code = %d, want CloseHandshakeRefused (%d)", appErr.ErrorCode, CloseHandshakeRefused)
	}
}

// TestASecondLoginOnOneCidClosesTheFirstConnection 是端到端的顶号。
//
// router 那一层已经测过顶号本身；这里测的是传输层真的把 Close 回调接上了。
// 接不上的话，旧连接会继续存在最长一个 MaxIdleTimeout，那位成员会听到
// 自己的声音——而 router 的测试完全看不到这一点。
func TestASecondLoginOnOneCidClosesTheFirstConnection(t *testing.T) {
	addr, priv, _ := testServer(t)
	tok, err := auth.Sign(priv, auth.Claims{
		CID: "1000", Rating: 5, MaxTX: 8, Exp: time.Now().Add(time.Minute).Unix(),
	})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}

	first := dial(t, addr)
	if _, m := hello(t, first, tok); func() bool { _, ok := m.(*control.Ready); return !ok }() {
		t.Fatalf("first handshake got %T, want *control.Ready", m)
	}

	second := dial(t, addr)
	if _, m := hello(t, second, tok); func() bool { _, ok := m.(*control.Ready); return !ok }() {
		t.Fatalf("second handshake got %T, want *control.Ready", m)
	}

	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		if first.Context().Err() != nil {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("the first connection is still open after the same cid signed in again; the transport did not wire up the eviction Close callback")
}

// TestEvictionUsesTheEvictedCloseCode 钉住顶号走的是 CloseEvicted。
//
// 客户端靠这个码来决定**不要重连**（见 codes.go 的注释）。它一旦漂移，症状是
// 两个客户端无限互顶，而服务端和客户端各自的测试都还是绿的——没有哪一边单独
// 看得出来。
func TestEvictionUsesTheEvictedCloseCode(t *testing.T) {
	addr, priv, _ := testServer(t)

	first := connect(t, addr, "1000", 8, priv)
	_ = connect(t, addr, "1000", 8, priv) // 同一个 CID，顶掉上面那条

	// 旧连接的上下文应当带着顶号的关闭码。
	ctx := first.conn.Context()
	select {
	case <-ctx.Done():
	case <-time.After(3 * time.Second):
		t.Fatal("the evicted connection was never closed")
	}
	var appErr *quic.ApplicationError
	if !errors.As(context.Cause(ctx), &appErr) {
		t.Fatalf("evicted connection closed with %v, want a *quic.ApplicationError", context.Cause(ctx))
	}
	if appErr.ErrorCode != CloseEvicted {
		t.Fatalf("close code = %d, want CloseEvicted (%d) — the client keys its do-not-reconnect rule on this exact value", appErr.ErrorCode, CloseEvicted)
	}
	// 原因串也是协议的一部分。它此前是一句内联的英文散文，挂在**客户端唯一一个
	// 必须遵守的关闭码**上——一个既看码又看串的客户端是完全合理的写法，而改一改
	// 措辞就把它弄坏了，还没有任何东西会响。
	if appErr.ErrorMessage != ReasonEvicted {
		t.Fatalf("reason = %q, want %q — this string is a literal a client may match on, not a sentence to reword", appErr.ErrorMessage, ReasonEvicted)
	}
}

// TestMaxRXIsEnforcedOverTheWire 钉住 Config.MaxRX 真的到了 router。
// 原文只在 READY 里把它告诉客户端，从不传下去——那个数字于是只是一句建议。
func TestMaxRXIsEnforcedOverTheWire(t *testing.T) {
	addr, priv, _ := testServer(t) // testServer 的 MaxRX 是 32
	tok, err := auth.Sign(priv, auth.Claims{
		CID: "1000", Rating: 5, MaxTX: 2, Exp: time.Now().Add(time.Minute).Unix(),
	})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	conn := dial(t, addr)
	st, m := hello(t, conn, tok)
	ready, ok := m.(*control.Ready)
	if !ok {
		t.Fatalf("got %T, want *control.Ready", m)
	}
	if ready.MaxRX != 32 {
		t.Fatalf("READY.MaxRX = %d, want 32", ready.MaxRX)
	}

	// 声明 40 个频率，超过 32。
	rx := make([]uint32, 0, 40)
	for i := 0; i < 40; i++ {
		rx = append(rx, uint32(118000+i*25))
	}
	b, _ := control.Encode(&control.Sub{RX: rx})
	if err := control.WriteFrame(st, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	resp, err := control.ReadFrame(st)
	if err != nil {
		t.Fatalf("ReadFrame: %v", err)
	}
	dm, err := control.Decode(resp)
	if err != nil {
		t.Fatalf("Decode: %v", err)
	}
	ack, ok := dm.(*control.SubAck)
	if !ok {
		t.Fatalf("got %T, want *control.SubAck", dm)
	}
	if len(ack.RX) != 32 {
		t.Fatalf("accepted RX = %d, want 32 — the advertised MaxRX must actually be enforced, not merely announced", len(ack.RX))
	}
	if len(ack.Rejected) != 8 {
		t.Fatalf("Rejected = %d, want 8", len(ack.Rejected))
	}
}

// TestPingIsAnsweredWithPong 覆盖控制面循环剩下的那一个分支。
func TestPingIsAnsweredWithPong(t *testing.T) {
	addr, priv, _ := testServer(t)
	c := connect(t, addr, "1000", 8, priv)

	b, _ := control.Encode(&control.Ping{T: 1234})
	if err := control.WriteFrame(c.st, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	resp, err := control.ReadFrame(c.st)
	if err != nil {
		t.Fatalf("ReadFrame: %v", err)
	}
	m, err := control.Decode(resp)
	if err != nil {
		t.Fatalf("Decode: %v", err)
	}
	pong, ok := m.(*control.Pong)
	if !ok {
		t.Fatalf("got %T, want *control.Pong", m)
	}
	if pong.T != 1234 {
		t.Fatalf("Pong.T = %d, want the client's 1234 echoed back — RTT is measured against it", pong.T)
	}
	if pong.ServerT == 0 {
		t.Fatal("Pong.ServerT must carry the server clock")
	}
}

// TestAnUndecodableControlFrameDoesNotKillTheSession 钉住垃圾帧只被跳过。
//
// 断开会话是错的答案：一个发错一帧的客户端会被踢下线，而它的语音链路本来
// 好端端的。这里同时证明这条断言的前提——会话在那之后**确实还活着**，
// 靠的是紧接着一个正常 SUB 仍然拿得到 SUBACK。
//
// 连 JSON 都不是的帧走的是和未知类型同一条路径，所以它同样换来一条 NOTICE。
// 那条 NOTICE 的 Reason 里**不能**出现对端发来的字节：报文可以有几十 KB，
// 原样回显既没必要也没好处。
func TestAnUndecodableControlFrameDoesNotKillTheSession(t *testing.T) {
	addr, priv, _ := testServer(t)
	c := connect(t, addr, "1000", 8, priv)

	garbage := "this is not json"
	if err := control.WriteFrame(c.st, []byte(garbage)); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	b, _ := control.Encode(&control.Sub{RX: []uint32{118000}})
	if err := control.WriteFrame(c.st, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}

	n := readNotice(t, c, "an undecodable frame must be answered too")
	if n.Kind != control.KindUnknownMessage {
		t.Fatalf("NOTICE.Kind = %q, want %q", n.Kind, control.KindUnknownMessage)
	}
	if n.Reason == "" {
		t.Fatal("NOTICE.Reason is empty; a server that cannot say what it disliked is as hard to debug as silence")
	}
	if strings.Contains(n.Reason, garbage) {
		t.Fatalf("NOTICE.Reason = %q echoes the frame back; it must carry the type string, not the client's bytes", n.Reason)
	}

	resp, err := control.ReadFrame(c.st)
	if err != nil {
		t.Fatalf("the session died on an undecodable frame: %v", err)
	}
	m, err := control.Decode(resp)
	if err != nil {
		t.Fatalf("Decode: %v", err)
	}
	if _, ok := m.(*control.SubAck); !ok {
		t.Fatalf("got %T, want *control.SubAck", m)
	}
}

// TestTheCloseCodesAndReasonsAreTheLiteralValuesTheProtocolNames 钉住那几个**数字和
// 字符串本身**。
//
// 别的测试写的是 `appErr.ErrorCode != CloseEvicted`，那是拿常量比常量——把
// CloseEvicted 从 2 改成 3，一条测试都不会红。而给它们起名字的**全部理由**就是
// 客户端那边会写死那个字面值：服务端改了、客户端没改，症状是无限互顶重新出现，
// 两边的测试各自还是绿的。所以这里比的是字面值。
func TestTheCloseCodesAndReasonsAreTheLiteralValuesTheProtocolNames(t *testing.T) {
	for _, tc := range []struct {
		name string
		got  quic.ApplicationErrorCode
		want uint64
	}{
		{"CloseNormal", CloseNormal, 0},
		{"CloseHandshakeRefused", CloseHandshakeRefused, 1},
		{"CloseEvicted", CloseEvicted, 2},
		{"CloseProtocolViolation", CloseProtocolViolation, 3},
	} {
		if uint64(tc.got) != tc.want {
			t.Errorf("%s = %d, want the wire value %d — clients hardcode this number", tc.name, tc.got, tc.want)
		}
	}
	for _, tc := range []struct {
		name, got, want string
	}{
		{"ReasonTokenExpired", ReasonTokenExpired, "token_expired"},
		{"ReasonTokenInvalid", ReasonTokenInvalid, "token_invalid"},
		{"ReasonRefused", ReasonRefused, "refused"},
		{"ReasonProtoUnsupported", ReasonProtoUnsupported, "proto_unsupported"},
		{"ReasonEvicted", ReasonEvicted, "evicted"},
		{"ReasonControlWriteStalled", ReasonControlWriteStalled, "control_write_stalled"},
		{"ReasonControlReadStalled", ReasonControlReadStalled, "control_read_stalled"},
		{"ReasonAckUndeliverable", ReasonAckUndeliverable, "ack_undeliverable"},
	} {
		if tc.got != tc.want {
			t.Errorf("%s = %q, want the wire value %q — clients match on this string", tc.name, tc.got, tc.want)
		}
	}
}

// TestTheAlpnAndProtoVersionAreTheLiteralValuesTheProtocolNames 是上一条的同款，
// 管另外两个跨实现字面值。
//
// ALPN 之前**哪里都没有钉**：所有测试都是 `NextProtos: []string{ALPN}`，
// 也就是拿常量比常量——把它改成 `can-voice/2`，整套测试照绿，而每一个已经装在
// 别人机器上的客户端都会在 TLS 握手上被拒（`no application protocol`），
// 一条日志都指不到这里。ProtoVersion 一样：握手那道闸判的是
// `h.Proto != control.ProtoVersion`，两边一起变就还是绿的。
//
// 这两个值和关闭码一样是"服务端改了、客户端没改"型的东西，区别只在于坏起来
// 是响亮的（连不上）而不是安静的。响亮不等于不用钉：响亮的是**用户**那边，
// 这边仍然什么都不会红。
func TestTheAlpnAndProtoVersionAreTheLiteralValuesTheProtocolNames(t *testing.T) {
	if ALPN != "can-voice/1" {
		t.Errorf("ALPN = %q, want the wire value %q — every shipped client puts this exact string in its TLS NextProtos", ALPN, "can-voice/1")
	}
	if control.ProtoVersion != 1 {
		t.Errorf("control.ProtoVersion = %d, want the wire value 1 — clients put this number in HELLO.proto", control.ProtoVersion)
	}
}

// TestADelayedReaderStillLearnsWhyItWasRefused 钉住"被拒的原因走的是关闭码那条道"。
//
// byeGrace 只是一个调度上的赌注：它赌对端此刻正卡在 ReadFrame 上。这个包里别的
// 测试全都是那样写的，所以它们看不见赌输的情形。这条不一样——它建连、发 HELLO，
// 然后 500 毫秒**不去读**（一个先去开音频设备、再回来读控制流的客户端就是这样），
// BYE 于是被服务端自己的 CONNECTION_CLOSE 吃掉。
//
// 而 ApplicationError 的 reason phrase 和关闭码同属一个帧，原子送达，丢不掉。
// 所以客户端真正该拿来判断的是它。
func TestADelayedReaderStillLearnsWhyItWasRefused(t *testing.T) {
	addr, priv, _ := testServer(t)
	tok, err := auth.Sign(priv, auth.Claims{
		CID: "1000", Rating: 5, MaxTX: 8, Exp: time.Now().Add(-time.Minute).Unix(),
	})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}

	conn := dial(t, addr)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	st, err := conn.OpenStreamSync(ctx)
	if err != nil {
		t.Fatalf("OpenStreamSync: %v", err)
	}
	b, _ := control.Encode(&control.Hello{Token: tok, Client: "test/1", Proto: 1})
	if err := control.WriteFrame(st, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}

	// 故意不读。byeGrace 是 200 毫秒。
	time.Sleep(500 * time.Millisecond)

	select {
	case <-conn.Context().Done():
	case <-time.After(3 * time.Second):
		t.Fatal("the refused connection was never closed")
	}
	var appErr *quic.ApplicationError
	if !errors.As(context.Cause(conn.Context()), &appErr) {
		t.Fatalf("closed with %v, want a *quic.ApplicationError", context.Cause(conn.Context()))
	}
	if appErr.ErrorCode != CloseHandshakeRefused {
		t.Fatalf("close code = %d, want CloseHandshakeRefused", appErr.ErrorCode)
	}
	if appErr.ErrorMessage != ReasonTokenExpired {
		t.Fatalf("close reason = %q, want %q — this client missed the BYE, and the close reason is the only channel that cannot be dropped; without it it cannot tell \"go get a new token\" from \"stop trying\"",
			appErr.ErrorMessage, ReasonTokenExpired)
	}

	// 这条测试的价值全在"BYE 确实没到"。万一它到了，上面的断言仍然成立，
	// 但这条测试就退化成一条普通的关闭码测试——记一笔，不判失败。
	if _, err := control.ReadFrame(st); err == nil {
		t.Log("note: the BYE survived the race this time; the assertion above still rests on the close reason")
	}
}

// TestTheControlStreamOutlivesTheHandshakeDeadline 钉住握手之后那两个截止时间被清掉。
//
// 不清的话，控制流会在连上 handshakeTimeout 之后自己死掉：不再有 PING、不再有 SUB，
// 会话被摘除——而一个几分钟不说话的管制员是完全正常的。客户端那边看不到任何事件，
// 只是忽然谁也听不见它了。
func TestTheControlStreamOutlivesTheHandshakeDeadline(t *testing.T) {
	orig := handshakeTimeout.Set(300 * time.Millisecond)
	defer handshakeTimeout.Set(orig)

	addr, priv, _ := testServer(t)
	c := connect(t, addr, "1000", 8, priv)

	// 睡过握手时限。
	time.Sleep(600 * time.Millisecond)

	// 客户端这边也要有截止时间，否则服务端真的不回时这条测试会挂到 go test
	// 的总超时，而不是给出一句话。
	_ = c.st.SetReadDeadline(time.Now().Add(3 * time.Second))

	b, _ := control.Encode(&control.Ping{T: 99})
	if err := control.WriteFrame(c.st, b); err != nil {
		t.Fatalf("writing on the control stream failed after the handshake deadline passed: %v", err)
	}
	resp, err := control.ReadFrame(c.st)
	if err != nil {
		// 读和写两个截止时间都要清，这一条两边都钉得住：留着读截止时间，
		// 上面那句 WriteFrame 就先炸了；留着写截止时间，服务端的 PONG 写不出去
		// （那个错误被刻意忽略），这里读到超时。
		t.Fatalf("no PONG after the handshake deadline passed: %v — one of the two post-handshake deadline clears is missing, so every session goes deaf %v after connecting", err, 300*time.Millisecond)
	}
	m, err := control.Decode(resp)
	if err != nil {
		t.Fatalf("Decode: %v", err)
	}
	pong, ok := m.(*control.Pong)
	if !ok {
		t.Fatalf("got %T, want *control.Pong", m)
	}
	if pong.T != 99 {
		t.Fatalf("Pong.T = %d, want 99", pong.T)
	}
}

// TestMaxTXIsEnforcedOverTheWire 是 TestMaxRXIsEnforcedOverTheWire 的另一半。
//
// MaxRX 是资源上限，MaxTX 是**授权**——它由成员的 rating 推出来，写在已验签的
// token 里。只断言 READY 里的回显等于什么都没断言：一张只授权一个发送频率的
// token 照样能在无限多个频率上发射，而回显那个数字始终是对的。
func TestMaxTXIsEnforcedOverTheWire(t *testing.T) {
	addr, priv, r := testServer(t)
	tok, err := auth.Sign(priv, auth.Claims{
		CID: "1000", Rating: 5, MaxTX: 2, Exp: time.Now().Add(time.Minute).Unix(),
	})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	st, m := hello(t, dial(t, addr), tok)
	ready, ok := m.(*control.Ready)
	if !ok {
		t.Fatalf("got %T, want *control.Ready", m)
	}
	if ready.MaxTX != 2 {
		t.Fatalf("READY.MaxTX = %d, want the token's 2", ready.MaxTX)
	}

	tx := []uint32{118000, 118025, 118050, 118075, 118100}
	b, _ := control.Encode(&control.Sub{TX: tx})
	if err := control.WriteFrame(st, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	resp, err := control.ReadFrame(st)
	if err != nil {
		t.Fatalf("ReadFrame: %v", err)
	}
	dm, err := control.Decode(resp)
	if err != nil {
		t.Fatalf("Decode: %v", err)
	}
	ack, ok := dm.(*control.SubAck)
	if !ok {
		t.Fatalf("got %T, want *control.SubAck", dm)
	}
	if len(ack.TX) != 2 {
		t.Fatalf("accepted TX = %d, want 2 — max_tx is an authorization claim and must be enforced, not merely echoed", len(ack.TX))
	}
	if len(ack.Rejected) != 3 {
		t.Fatalf("Rejected = %d, want 3", len(ack.Rejected))
	}

	// 而且不能只是 ACK 上好看：被拒的频率上真的不能发。
	id := router.SessionID(ready.Session)
	for _, f := range ack.Rejected {
		if r.MayTransmit(id, f) {
			t.Fatalf("%d was rejected in the ACK but the router still lets the session transmit on it", f)
		}
	}
	// 缺席断言的前提：被接受的那两个确实能发，否则上面那句可能只是因为
	// 这条会话在**任何**频率上都不能发。
	for _, f := range ack.TX {
		if !r.MayTransmit(id, f) {
			t.Fatalf("premise failed: %d was granted in the ACK but the router refuses it too, so the assertion above proved nothing", f)
		}
	}
}

// TestTheFollowFieldFromHelloReachesTheSession 钉住观察员的跟随目标不会在握手里丢掉。
//
// 丢了的话，观察员的 Follow 是空串，扇出就拿不到它该用的那架飞机的位置——
// 射程于是按"位置未知"算，而那条路径是放行。症状是观察员听得见本不该听见的
// 远处电台，没有任何错误。
func TestTheFollowFieldFromHelloReachesTheSession(t *testing.T) {
	addr, priv, r := testServer(t)
	tok, err := auth.Sign(priv, auth.Claims{
		CID: "1000", Rating: 5, MaxTX: 8, Exp: time.Now().Add(time.Minute).Unix(),
	})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}

	conn := dial(t, addr)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	st, err := conn.OpenStreamSync(ctx)
	if err != nil {
		t.Fatalf("OpenStreamSync: %v", err)
	}
	b, _ := control.Encode(&control.Hello{Token: tok, Client: "test/1", Proto: 1, Follow: "CCA101"})
	if err := control.WriteFrame(st, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	resp, err := control.ReadFrame(st)
	if err != nil {
		t.Fatalf("ReadFrame: %v", err)
	}
	m, err := control.Decode(resp)
	if err != nil {
		t.Fatalf("Decode: %v", err)
	}
	ready, ok := m.(*control.Ready)
	if !ok {
		t.Fatalf("got %T, want *control.Ready", m)
	}
	sess, ok := r.Get(router.SessionID(ready.Session))
	if !ok {
		t.Fatal("the session was not registered")
	}
	if sess.Follow != "CCA101" {
		t.Fatalf("Session.Follow = %q, want %q from the HELLO", sess.Follow, "CCA101")
	}
}

// TestAcceptReportsAListenerFailureRatherThanReturningNil 钉住监听器挂掉会被报上去。
//
// 返回 nil 的话 Serve 也返回 nil，整个进程静悄悄地"正常退出"，而端口其实
// 已经没人在听了——没有任何一行日志说出这件事。
func TestAcceptReportsAListenerFailureRatherThanReturningNil(t *testing.T) {
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

	dead, err := listen(cfg)
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	dead.Close()
	// ctx 没有取消：这是"监听器自己死了"，不是收摊。
	if err := accept(context.Background(), dead, cfg, router.New(), newConnSet()); err == nil {
		t.Fatal("accept returned nil after the listener died; Serve would then report a clean shutdown while nothing is listening")
	}

	// 另一半，也是上面那条断言的前提：ctx 取消时 accept 报的必须是 ctx 的错误，
	// 而不是把正常收摊也说成故障——不然上面那个 non-nil 什么都证明不了。
	live, err := listen(cfg)
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	defer live.Close()
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if err := accept(ctx, live, cfg, router.New(), newConnSet()); !errors.Is(err, context.Canceled) {
		t.Fatalf("accept on a cancelled ctx returned %v, want context.Canceled", err)
	}
}

// TestAnUnknownMessageTypeGetsANoticeAndKeepsTheSession 钉住这条协议决定的两半。
// 它们互为对方的前提，所以在同一条测试里（这条是原来那个只钉"不断开"的加强版）。
//
// **不断开**：一个比服务端新一个协议版本的客户端应该降级，不该掉线——而且断开是
// 不对称的，服务端没法解释为什么，客户端看到的只是掉线，于是它重连、重发。
//
// **但也不静默**：客户端发了一条消息、什么都没发生、又不知道为什么，是这个项目
// 在别处明确拒绝的失败形态（SubAck.RejectedXC 就是为这条原则存在的）。所以回一条
// NOTICE，Kind 是 unknown_message，Reason 是那个**类型字符串**而不是原始报文。
func TestAnUnknownMessageTypeGetsANoticeAndKeepsTheSession(t *testing.T) {
	addr, priv, _ := testServer(t)
	c := connect(t, addr, "1000", 8, priv)

	if err := control.WriteFrame(c.st, []byte(`{"type":"NOPE"}`)); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}

	n := readNotice(t, c, "an unknown message type must be answered")
	if n.Kind != control.KindUnknownMessage {
		t.Fatalf("NOTICE.Kind = %q, want %q", n.Kind, control.KindUnknownMessage)
	}
	// Reason 是类型字符串，不是原始报文——报文可能有几十 KB，回显它没有用处。
	if n.Reason != "NOPE" {
		t.Fatalf("NOTICE.Reason = %q, want the type string %q", n.Reason, "NOPE")
	}

	// 另一半：会话还活着。PING/PONG 证明的是"服务端还在读这条流"，
	// 比"连接还没关"强。
	if pong := ping(t, c); pong.T != 42 {
		t.Fatalf("PONG.T = %d, want the 42 we sent — the session died on an unknown message type", pong.T)
	}
}

// TestTheNoticeBudgetForUndecodableFramesIsBounded 钉住那条 NOTICE 不是无限的。
//
// 回 NOTICE 走的是控制流，而 readControl 在同一个 goroutine 里读和写：一个只发
// 不读的对端能让 QUIC 的流级流控把写顶住，那条会话就此不再读任何东西，从外面看
// 却完全正常。上限把这条路堵死。
//
// 这是一条否定式断言（"第 9 条不来"），所以它自带前提的证明：预算之内的
// noticeBudget 条必须一条不少地收到，否则"没有更多了"可能只是因为一条都没有。
func TestTheNoticeBudgetForUndecodableFramesIsBounded(t *testing.T) {
	addr, priv, _ := testServer(t)
	c := connect(t, addr, "1000", 8, priv)

	const extra = 2
	for i := 0; i < noticeBudget+extra; i++ {
		if err := control.WriteFrame(c.st, []byte(`{"type":"NOPE"}`)); err != nil {
			t.Fatalf("WriteFrame %d: %v", i, err)
		}
	}

	for i := 0; i < noticeBudget; i++ {
		n := readNotice(t, c, fmt.Sprintf("notice %d of %d is inside the budget", i+1, noticeBudget))
		if n.Kind != control.KindUnknownMessage {
			t.Fatalf("notice %d: Kind = %q, want %q", i+1, n.Kind, control.KindUnknownMessage)
		}
	}

	// 超出预算的那两条必须换来静默：紧接着的 PING 的应答必须**就是** PONG，
	// 中间不能再夹着 NOTICE。
	if pong := ping(t, c); pong.T != 42 {
		t.Fatalf("PONG.T = %d, want 42", pong.T)
	}
}

// replyWait 是等一条应答的上限。
//
// 必须有：这几条测试断言的正是"服务端会回点什么"，而一个不回的服务端会让
// ReadFrame 永远阻塞——红是红了，红出来的却是整包超时后的 goroutine dump，
// 而不是那一句说明错在哪的话。实测过：把回 NOTICE 那一步删掉，没有这个截止
// 时间就是 `panic: test timed out after 1m0s`。
const replyWait = 3 * time.Second

// readNotice 读下一帧并要求它是 NOTICE。
func readNotice(t *testing.T, c *client, why string) *control.Notice {
	t.Helper()
	_ = c.st.SetReadDeadline(time.Now().Add(replyWait))
	defer func() { _ = c.st.SetReadDeadline(time.Time{}) }()
	raw, err := control.ReadFrame(c.st)
	if err != nil {
		t.Fatalf("%s, but the read failed: %v — the session may have been dropped instead", why, err)
	}
	m, err := control.Decode(raw)
	if err != nil {
		t.Fatalf("Decode: %v", err)
	}
	n, ok := m.(*control.Notice)
	if !ok {
		t.Fatalf("%s: got %T, want *control.Notice", why, m)
	}
	return n
}

// ping 发一个 PING 并要求**下一帧**就是它的 PONG。
//
// 拿它做"会话还活着"的证明，比再发一个 SUB 强一点：它同时证明中间没有夹别的帧，
// 而那正是预算那条测试要的。
func ping(t *testing.T, c *client) *control.Pong {
	t.Helper()
	b, err := control.Encode(&control.Ping{T: 42})
	if err != nil {
		t.Fatalf("Encode PING: %v", err)
	}
	if err := control.WriteFrame(c.st, b); err != nil {
		t.Fatalf("WriteFrame PING: %v", err)
	}
	_ = c.st.SetReadDeadline(time.Now().Add(replyWait))
	defer func() { _ = c.st.SetReadDeadline(time.Time{}) }()
	raw, err := control.ReadFrame(c.st)
	if err != nil {
		t.Fatalf("the session did not answer a PING: %v", err)
	}
	m, err := control.Decode(raw)
	if err != nil {
		t.Fatalf("Decode: %v", err)
	}
	pong, ok := m.(*control.Pong)
	if !ok {
		t.Fatalf("after PING got %T, want *control.Pong", m)
	}
	return pong
}

// 本段钉住"写把读堵死"那条路已经被封上（Task 9C）。
//
// 形状：readControl 在同一个 goroutine 里读和写，而
// initial_max_stream_data_bidi_remote 是**对端**的传输参数、quic-go v0.48.2 对它
// 不设下限。一个握完手就不再读、并且把接收窗口调得很小的对端，会让服务端的下一次
// 控制面写永久阻塞在流控上——于是这条会话之后的 SUB 一条都不会被处理，客户端继续
// 收着旧的那套频率，台面改动看上去毫无反应，连接却一切正常。

// stalledWindow 是那个不读的对端通告的流接收窗口。
//
// quic-go 的 populateConfig 对 InitialStreamReceiveWindow 只在为 0 时填默认值，
// 不设下限（config.go:75-78），所以这个数能真的小到几帧就填满。
// MaxStreamReceiveWindow 一起钉住，否则自动扩窗会把窗口抬起来。
const stalledWindow = 1024

// dialWindow 和 dial 一样，但让调用方指定流接收窗口。
//
// 刻意不去改 helpers_test.go 的 dial：那是 Task 10 也在用的共享脚手架。
func dialWindow(t *testing.T, addr string, window uint64) quic.Connection {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	t.Cleanup(cancel)
	conn, err := quic.DialAddr(ctx, addr, &tls.Config{
		InsecureSkipVerify: true,
		NextProtos:         []string{ALPN},
	}, &quic.Config{
		EnableDatagrams:            true,
		InitialStreamReceiveWindow: window,
		MaxStreamReceiveWindow:     window,
	})
	if err != nil {
		t.Fatalf("DialAddr: %v", err)
	}
	t.Cleanup(func() { conn.CloseWithError(0, "") })
	return conn
}

// TestAPeerThatStopsReadingItsControlStreamIsClosed 钉住卡住的写会把连接关掉，
// 而且关成客户端能认出来的那个码。
//
// 对端是真的用 quic-go 造出来的：握手走完（所以它读过 READY），之后一个字都不读，
// 并且它自己通告了一个 1 KB 的流接收窗口。灌 SUB，每条换一条 SUBACK，几条之后
// 服务端的写就卡在流控上。
func TestAPeerThatStopsReadingItsControlStreamIsClosed(t *testing.T) {
	restore := controlWriteTimeout.Set(300 * time.Millisecond)
	defer controlWriteTimeout.Set(restore)

	addr, priv, _ := testServer(t)
	tok, err := auth.Sign(priv, auth.Claims{
		CID: "1000", Rating: 5, MaxTX: 8, Exp: time.Now().Add(time.Minute).Unix(),
	})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	conn := dialWindow(t, addr, stalledWindow)
	st, m := hello(t, conn, tok)
	if _, ok := m.(*control.Ready); !ok {
		t.Fatalf("handshake got %T, want *control.Ready", m)
	}

	// 从这里开始这个对端一个字都不读。每条 SUB 换一条 SUBACK，
	// 频率填满 MaxRX 让每条回复都尽量大。
	freqs := make([]uint32, 0, 32)
	for i := 0; i < 32; i++ {
		freqs = append(freqs, uint32(118000+i*25))
	}
	b, err := control.Encode(&control.Sub{RX: freqs})
	if err != nil {
		t.Fatalf("Encode SUB: %v", err)
	}
	for i := 0; i < 64; i++ {
		if err := control.WriteFrame(st, b); err != nil {
			break // 服务端已经把连接关了，这正是我们要的
		}
	}

	ctx := conn.Context()
	select {
	case <-ctx.Done():
	case <-time.After(10 * time.Second):
		t.Fatal("a peer that stopped reading its control stream was never closed; that session is now silently deaf to every later SUB while looking perfectly healthy")
	}
	var appErr *quic.ApplicationError
	if !errors.As(context.Cause(ctx), &appErr) {
		t.Fatalf("connection closed with %v, want a *quic.ApplicationError", context.Cause(ctx))
	}
	if appErr.ErrorCode != CloseProtocolViolation {
		t.Fatalf("close code = %d, want CloseProtocolViolation (%d) — CloseNormal here would tell the client to reconnect, and reconnecting replays the same bug", appErr.ErrorCode, CloseProtocolViolation)
	}
	if appErr.ErrorMessage != ReasonControlWriteStalled {
		t.Fatalf("reason = %q, want %q", appErr.ErrorMessage, ReasonControlWriteStalled)
	}
}

// TestTheControlWriteDeadlineDoesNotFireForAHealthyClient 是上一条的前提证明。
//
// 上一条只说明"某个对端被关了"。如果那个截止时间对**每个人**都开火，它照样通过，
// 而服务端会在生产上把所有人都掐掉。这里用**同一个**被调小的时限跑一个正常读的
// 客户端：同样的 SUB、同样的往返次数，必须一次都不开火。
//
// 同一个时限是关键——换成一个更宽松的值，这条测试就什么都不证明了。
func TestTheControlWriteDeadlineDoesNotFireForAHealthyClient(t *testing.T) {
	restore := controlWriteTimeout.Set(300 * time.Millisecond)
	defer controlWriteTimeout.Set(restore)

	addr, priv, _ := testServer(t)
	c := connect(t, addr, "1000", 8, priv)

	freqs := make([]uint32, 0, 32)
	for i := 0; i < 32; i++ {
		freqs = append(freqs, uint32(118000+i*25))
	}
	for i := 0; i < 32; i++ {
		c.subscribe(t, control.Sub{RX: freqs})
		if pong := ping(t, c); pong.T != 42 {
			t.Fatalf("round %d: PONG.T = %d, want 42", i, pong.T)
		}
	}

	select {
	case <-c.conn.Context().Done():
		t.Fatalf("the write deadline fired for a client that was reading normally: %v — in production that closes every healthy session", context.Cause(c.conn.Context()))
	default:
	}
}

// TestTransmittingOnAnUndeclaredFrequencyGetsANotice 钉住"按了 PTT 没人听见"
// 至少要换来一句话。
//
// 在这条测试之前服务端把这一帧丢掉，只写一行 Debug 日志。客户端 pump.rs 有一条
// 完整的 tx_denied 接收分支，而它是死代码——服务端从来没发过。症状是一个在没有
// 声明 TX 的频率上按住 PTT 的管制员，界面、日志、对端三处都看不出任何异常。
func TestTransmittingOnAnUndeclaredFrequencyGetsANotice(t *testing.T) {
	addr, priv, _ := testServer(t)
	c := connect(t, addr, "1000", 8, priv)
	// 只听，不发。
	c.subscribe(t, control.Sub{RX: []uint32{118000}})

	pkt := append(wire.Header{
		Ver: wire.Version, Flags: wire.FlagFirst, Seq: 1, FreqKHz: 118000,
	}.AppendTo(nil), 0x01, 0x02, 0x03, 0x04)
	if err := c.conn.SendDatagram(pkt); err != nil {
		t.Fatalf("SendDatagram: %v", err)
	}

	n := readNotice(t, c, "transmitting on a frequency the session never declared must be answered")
	if n.Kind != control.KindTxDenied {
		t.Fatalf("NOTICE.Kind = %q, want %q", n.Kind, control.KindTxDenied)
	}
	if n.Freq != 118000 {
		t.Fatalf("NOTICE.Freq = %d, want 118000 — a client with several radios cannot tell which one was refused", n.Freq)
	}
}

// TestTheTxDeniedNoticeIsThrottled 钉住这条 NOTICE 自己不会变成一场拒绝服务。
//
// 按住 PTT 的客户端每秒发 50 个包。一包一条 NOTICE 的话控制流上全是它，
// 而 SUBACK 和 PONG 要排在后面——那是拿一个静默失败换一个更响的故障。
//
// "只有一条"的证明是**下一帧必须是 PONG**：中间再夹一条 tx_denied，
// ping 就会失败。
func TestTheTxDeniedNoticeIsThrottled(t *testing.T) {
	addr, priv, _ := testServer(t)
	c := connect(t, addr, "1000", 8, priv)
	c.subscribe(t, control.Sub{RX: []uint32{118000}})

	pkt := append(wire.Header{
		Ver: wire.Version, Flags: wire.FlagFirst, Seq: 1, FreqKHz: 118000,
	}.AppendTo(nil), 0x01, 0x02, 0x03, 0x04)
	for i := 0; i < 20; i++ {
		if err := c.conn.SendDatagram(pkt); err != nil {
			t.Fatalf("SendDatagram %d: %v", i, err)
		}
	}

	n := readNotice(t, c, "the first refused packet must be answered")
	if n.Kind != control.KindTxDenied {
		t.Fatalf("NOTICE.Kind = %q, want %q", n.Kind, control.KindTxDenied)
	}
	ping(t, c)
}

// TestTransmittingWhileRangeFilteringIsDegradedSaysSo 钉住降级不是一件悄悄发生的事。
//
// 位置快照取不到时服务端**全部放行**——射程过滤整个不生效，一个塔台频率上的话
// 会传到全国。这是正确的取舍（语音能不能通比射程真实感重要），但它必须说出来：
// 没有这条 NOTICE 的话，两端都看不出今天和昨天有什么不同。
//
// testServer 不装 Locator，所以它就是永久降级的那一种。
func TestTransmittingWhileRangeFilteringIsDegradedSaysSo(t *testing.T) {
	addr, priv, _ := testServer(t)
	c := connect(t, addr, "1000", 8, priv)
	c.subscribe(t, control.Sub{RX: []uint32{121800}, TX: []uint32{121800}})

	pkt := append(wire.Header{
		Ver: wire.Version, Flags: wire.FlagFirst, Seq: 1, FreqKHz: 121800,
	}.AppendTo(nil), 0x01, 0x02, 0x03, 0x04)
	if err := c.conn.SendDatagram(pkt); err != nil {
		t.Fatalf("SendDatagram: %v", err)
	}

	n := readNotice(t, c, "transmitting while range filtering is off must be answered")
	if n.Kind != control.KindRangeUnavailable {
		t.Fatalf("NOTICE.Kind = %q, want %q", n.Kind, control.KindRangeUnavailable)
	}

	// **一条会话只说一次。** 降级会持续几分钟甚至几小时，每一帧说一次
	// 就是每秒五十次。下一帧必须是 PONG。
	if err := c.conn.SendDatagram(pkt); err != nil {
		t.Fatalf("SendDatagram: %v", err)
	}
	ping(t, c)
}

// 通播机队整队共用一个 CID，每一路席位靠 HELLO 里的 station 区分。
// 这个字段要真的落到会话上：router 的顶号键读的是它，读不到就等于没有。
func TestTheStationFieldFromHelloReachesTheSession(t *testing.T) {
	addr, priv, r := testServer(t)
	tok, err := auth.Sign(priv, auth.Claims{
		CID: "1000", Rating: 5, MaxTX: 8, Exp: time.Now().Add(time.Minute).Unix(),
	})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}

	conn := dial(t, addr)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	st, err := conn.OpenStreamSync(ctx)
	if err != nil {
		t.Fatalf("OpenStreamSync: %v", err)
	}
	b, _ := control.Encode(&control.Hello{Token: tok, Client: "test/1", Proto: 1, Station: "ZSPD_ATIS"})
	if err := control.WriteFrame(st, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	resp, err := control.ReadFrame(st)
	if err != nil {
		t.Fatalf("ReadFrame: %v", err)
	}
	m, err := control.Decode(resp)
	if err != nil {
		t.Fatalf("Decode: %v", err)
	}
	ready, ok := m.(*control.Ready)
	if !ok {
		t.Fatalf("got %T, want *control.Ready", m)
	}
	sess, ok := r.Get(router.SessionID(ready.Session))
	if !ok {
		t.Fatal("the session was not registered")
	}
	if sess.Station != "ZSPD_ATIS" {
		t.Fatalf("Session.Station = %q, want %q from the HELLO", sess.Station, "ZSPD_ATIS")
	}
}

// station 进的是顶号表的键，所以和 follow 一样要先校形状——
// 不校的话一个 6 万字节的值就那么进去了，而且每一条这样的会话都顶不掉任何人。
func TestAStationThatIsNotACallsignIsRefused(t *testing.T) {
	addr, priv, r := testServer(t)
	tok, err := auth.Sign(priv, auth.Claims{
		CID: "1000", Rating: 5, MaxTX: 8, Exp: time.Now().Add(time.Minute).Unix(),
	})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}

	conn := dial(t, addr)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	st, err := conn.OpenStreamSync(ctx)
	if err != nil {
		t.Fatalf("OpenStreamSync: %v", err)
	}
	b, _ := control.Encode(&control.Hello{
		Token: tok, Client: "test/1", Proto: 1,
		Station: strings.Repeat("A", 64),
	})
	if err := control.WriteFrame(st, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	resp, err := control.ReadFrame(st)
	if err != nil {
		t.Fatalf("ReadFrame: %v", err)
	}
	m, err := control.Decode(resp)
	if err != nil {
		t.Fatalf("Decode: %v", err)
	}
	if _, ok := m.(*control.Bye); !ok {
		t.Fatalf("got %T, want *control.Bye for a malformed station", m)
	}
	if n := r.SessionCount(); n != 0 {
		t.Fatalf("SessionCount() = %d, want 0: a refused handshake must not leave a session", n)
	}
}

func noticeOfKind(t *testing.T, c *client, kind, why string) *control.Notice {
	t.Helper()
	const maxFrames = 8
	for range maxFrames {
		n := readNotice(t, c, why)
		if n.Kind == kind {
			return n
		}
		t.Logf("skipping a %q notice while waiting for %q", n.Kind, kind)
	}
	t.Fatalf("%s: read %d notices and none was %q", why, maxFrames, kind)
	return nil
}

// TestAFloodedUplinkIsRateLimitedAndTheSessionRecovers 是 uplink.go 的接线。
//
// 桶的算术在 uplink_test.go 里按注入的时间钉死了；这一条只证明三件事，而三件
// 都只有走真连接才看得见：readDatagrams **真的**问过那个桶、超额**真的**回一条
// NOTICE、以及超额之后这条会话**还活着并且还能说话**。
//
// 最后那半条是重点。限速拦的多半是客户端的一个 bug，不是攻击者；把它踢下线是
// 不对称的处理（对端只看得见掉线，于是重连、重放，回到同一个地方），所以正确的
// 结果是"丢掉多出来的那些，然后继续"。一个只断言"被限了"的测试，对"限完就再也
// 说不了话"这种实现照样是绿的。
func TestAFloodedUplinkIsRateLimitedAndTheSessionRecovers(t *testing.T) {
	addr, priv, _ := testServer(t)

	// max_tx 1：额度按可发射的频率数算，取 1 让突发额度是最小的那一档
	// （uplinkBurstFrames 帧），洪水才不需要发到几千个包才越线。
	speaker := connect(t, addr, "1000", 1, priv)
	listener := connect(t, addr, "1001", 8, priv)
	speaker.subscribe(t, control.Sub{TX: []uint32{118000}})
	listener.subscribe(t, control.Sub{RX: []uint32{118000}})

	pkt := func(seq uint16) []byte {
		return append(wire.Header{
			Ver: wire.Version, Flags: wire.FlagFirst, Seq: seq, FreqKHz: 118000,
		}.AppendTo(nil), 0x01, 0x02, 0x03, 0x04)
	}

	// 远多于突发额度，而且一口气发完——一个卡住的 PTT 就是这个形状。
	flood := 16 * uplinkBurstFrames
	for i := range flood {
		if err := speaker.conn.SendDatagram(pkt(1)); err != nil {
			t.Fatalf("SendDatagram %d: %v", i, err)
		}
	}

	n := noticeOfKind(t, speaker, control.KindRateLimited,
		"an uplink far above the Opus frame rate must be answered")
	if n.Reason == "" {
		t.Fatal("the rate_limited notice carries no reason; the client can show the member nothing but the word itself")
	}

	// 会话还在：下一帧是 PONG，不是 BYE，也不是断开。
	ping(t, speaker)

	// 而且它还能说话。等一帧的时间让桶回血，再发一个认得出的包，
	// 听众必须收到它。
	time.Sleep(50 * time.Millisecond)
	const recovered = 999
	if err := speaker.conn.SendDatagram(pkt(recovered)); err != nil {
		t.Fatalf("SendDatagram after the flood: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	for {
		got, err := listener.conn.ReceiveDatagram(ctx)
		if err != nil {
			t.Fatalf("the listener never heard the packet sent after the flood: %v — a rate-limited session that cannot speak again is stuck until it reconnects, and nothing tells it to", err)
		}
		h, _, err := wire.Parse(got)
		if err != nil {
			t.Fatalf("Parse: %v", err)
		}
		if h.Seq == recovered {
			return
		}
		// 洪水里被放行的那几帧还在路上，跳过它们。
	}
}
