package transport

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/tls"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/auth"
	"github.com/JianyueLab-Org/can-voice/server/internal/control"
	"github.com/JianyueLab-Org/can-voice/server/internal/router"
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
	go accept(ctx, ln, cfg, r)
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

func TestHelloWithABadTokenGetsByeAndNoSession(t *testing.T) {
	addr, priv, r := testServer(t)
	_, otherPriv, _ := ed25519.GenerateKey(rand.Reader)
	tok, _ := auth.Sign(otherPriv, auth.Claims{CID: "1000", Exp: time.Now().Add(time.Minute).Unix()})

	_, m := hello(t, dial(t, addr), tok)
	bye, ok := m.(*control.Bye)
	if !ok {
		t.Fatalf("server replied %T, want *control.Bye", m)
	}
	if bye.Reason == "" {
		t.Fatal("BYE must say why")
	}
	if _, ok := r.Get(router.SessionID(1)); ok {
		t.Fatal("a rejected HELLO must not leave a session behind")
	}

	// 上面那一条是一句**缺席断言**，而缺席断言要自己证明前提：r.Get 在这个
	// router 上确实会对一条真的存在的会话返回 true。不证的话，"Get 永远返回
	// false"（比如握手根本没接上 router）也会让它绿。
	good, err := auth.Sign(priv, auth.Claims{CID: "1001", MaxTX: 1, Exp: time.Now().Add(time.Minute).Unix()})
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
}

func TestSubGetsSubAckAndTakesEffect(t *testing.T) {
	addr, priv, r := testServer(t)
	tok, _ := auth.Sign(priv, auth.Claims{CID: "1000", MaxTX: 8, Exp: time.Now().Add(time.Minute).Unix()})
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
	tok, err := auth.Sign(priv, auth.Claims{CID: "1000", MaxTX: 1, Exp: time.Now().Add(time.Minute).Unix()})
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
	good, err := auth.Sign(priv, auth.Claims{CID: "1001", MaxTX: 1, Exp: time.Now().Add(time.Minute).Unix()})
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
func TestAnUndecodableControlFrameDoesNotKillTheSession(t *testing.T) {
	addr, priv, _ := testServer(t)
	c := connect(t, addr, "1000", 8, priv)

	if err := control.WriteFrame(c.st, []byte("this is not json")); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	b, _ := control.Encode(&control.Sub{RX: []uint32{118000}})
	if err := control.WriteFrame(c.st, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
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
