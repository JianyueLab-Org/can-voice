package transport

import (
	"bytes"
	"crypto/ed25519"
	"crypto/rand"
	"errors"
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
}

func (s *memStream) Read(p []byte) (int, error) { return s.in.Read(p) }

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
}

func (c *stubConn) SendDatagram([]byte) error { return nil }

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
// 观察点：如果真的漏了，同一个 CID 下一次登录会**顶掉**那条幽灵——而顶号会调它的
// Close 回调。所以 victim 有没有被 Close，就是"它是不是还在表里"。
func TestAFailedReadyWriteDoesNotLeakTheSession(t *testing.T) {
	pub, priv := testKeys(t)
	r := router.New()
	cfg := Config{PublicKey: pub, MaxRX: 32}

	victim := &stubConn{}
	if _, err := handshake(helloStream(t, priv, "1000", true), victim, cfg, r, func([]byte) {}); err == nil {
		t.Fatal("handshake must fail when READY cannot be written")
	}

	// 同一个 CID 再登录一次。
	if _, err := handshake(helloStream(t, priv, "1000", false), &stubConn{}, cfg, r, func([]byte) {}); err != nil {
		t.Fatalf("the second handshake failed: %v", err)
	}

	if waitClosed(victim, 300*time.Millisecond) {
		t.Fatal("the failed handshake left its session in the router: the next login on the same cid found one to evict")
	}
}

// TestTheEvictionProbeUsedByTheLeakTestActuallyFires 是上一条的前提证明。
//
// 上一条断言的是"victim 没有被 Close"。如果顶号根本就不会调 Close（比如回调没接上），
// 那句话恒真，什么都没证明。这里让第一次握手**成功**，同一个 CID 再来一次，
// Close 必须被调到。
func TestTheEvictionProbeUsedByTheLeakTestActuallyFires(t *testing.T) {
	pub, priv := testKeys(t)
	r := router.New()
	cfg := Config{PublicKey: pub, MaxRX: 32}

	victim := &stubConn{}
	if _, err := handshake(helloStream(t, priv, "1000", false), victim, cfg, r, func([]byte) {}); err != nil {
		t.Fatalf("first handshake: %v", err)
	}
	if _, err := handshake(helloStream(t, priv, "1000", false), &stubConn{}, cfg, r, func([]byte) {}); err != nil {
		t.Fatalf("second handshake: %v", err)
	}
	if !waitClosed(victim, 3*time.Second) {
		t.Fatal("premise failed: a second login on the same cid did not close the first connection at all, so the leak test above proves nothing")
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
