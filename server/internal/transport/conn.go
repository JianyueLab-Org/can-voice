package transport

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"sync/atomic"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/auth"
	"github.com/JianyueLab-Org/can-voice/server/internal/control"
	"github.com/JianyueLab-Org/can-voice/server/internal/router"
	"github.com/quic-go/quic-go"
)

// atomicDuration 是一个可以在服务端已经跑起来之后安全改动的时长。
//
// 下面两个时限都要在测试里被调小，而改它的那个 goroutine 和读它的 handleConn
// 从来不是同一个：前一条测试的 accept/handleConn 在自己的 t.Cleanup 之后还会
// 活一小会儿，正好撞上后一条测试的赋值。裸 var 在那里就是一条实打实的数据竞争
// （实测：`go test -race` 把整包染红），而那种红会盖住真正的并发缺陷。
type atomicDuration struct{ ns atomic.Int64 }

func newDuration(d time.Duration) *atomicDuration {
	a := &atomicDuration{}
	a.ns.Store(int64(d))
	return a
}

// Get 读当前值。
func (a *atomicDuration) Get() time.Duration { return time.Duration(a.ns.Load()) }

// Set 写入新值并返回旧值，测试里拿它 defer 还原。
func (a *atomicDuration) Set(d time.Duration) time.Duration {
	return time.Duration(a.ns.Swap(int64(d)))
}

// handshakeTimeout 是从连接建立到收下 HELLO 的总时限。
//
// 这是公网 UDP 端口。没有它，一个打开 QUIC 连接后什么都不发的对端会永久占住
// 一个 goroutine 和一条连接——不需要带宽，不需要有效凭据，开几千条就够了。
// 10 秒对一个真实客户端绰绰有余：它建连之后立刻就发 HELLO。
//
// 是变量而不是常量，只为了测试能把它调小（和 fsdfeed 的 feedIdleTimeout 同样
// 的理由）——生产代码不要改它。
var handshakeTimeout = newDuration(10 * time.Second)

// byeGrace 是写完 BYE 之后、真正关闭连接之前留给对端的读取窗口。
//
// 必须有，而且必须是"等一会儿"而不是"立刻关"：quic-go 收到 CONNECTION_CLOSE
// 之后会把已经到达、但还没有被应用读走的流数据整个丢掉——receive_stream.go 的
// readImpl 第一件事就是查 closeForShutdownErr，查到就返回错误，根本不看缓冲区里
// 还躺着什么。于是"WriteFrame(BYE) 紧接着 CloseWithError"是一场竞态：两个包前后脚
// 到达对端，对端的读协程只要没被调度到那条缝里，BYE 就没了。
//
// 而 BYE 正是客户端用来区分"去换一张 token 再试"和"别试了"的唯一信号——
// 关闭码只说"握手被拒"，说不出是哪一种。
//
// 代价有界且远小于已经允许的握手超时：一条被拒的连接多占这么久的一个 goroutine，
// 而一条一言不发的连接本来就能占满 handshakeTimeout。
//
// 同样是变量只为了测试能改它。
var byeGrace = newDuration(200 * time.Millisecond)

// handleConn 处理一条连接的完整生命周期。
func handleConn(ctx context.Context, conn quic.Connection, cfg Config, r *router.Router) {
	// 无论从哪条路径退出，连接都要关掉。没有这一条的话，readControl 返回、
	// 会话已摘除之后，readDatagrams 还在 ReceiveDatagram 上转，每个包都走一次
	// Fanout 拿到 "session is gone"——goroutine 泄漏到连接真正超时为止。
	//
	// 已经被别的路径关过的连接再关一次是空操作（quic-go 的 closeOnce），
	// 所以这条 defer 不会把 CloseHandshakeRefused / CloseEvicted 覆盖掉。
	defer conn.CloseWithError(CloseNormal, "")

	// 握手必须有超时。这是公网 UDP 端口：一个打开 QUIC 连接后什么都不发的
	// 对端，会永久占住一个 goroutine 和一条连接——slowloris。AcceptStream 和
	// 读第一帧两步都要盖住，所以超时挂在一个派生 ctx 上，并用流的读截止时间
	// 兜住第二步（ctx 取消不会打断一个已经阻塞在 Read 上的流）。
	hctx, hcancel := context.WithTimeout(ctx, handshakeTimeout.Get())
	defer hcancel()
	st, err := conn.AcceptStream(hctx)
	if err != nil {
		slog.Debug("no control stream before the handshake deadline",
			"peer", conn.RemoteAddr().String(), "error", err)
		return
	}
	deadline := time.Now().Add(handshakeTimeout.Get())
	_ = st.SetReadDeadline(deadline)
	// 写也要有截止时间。对端的传输参数是**它**说了算的，一个把
	// initial_max_stream_data_bidi_remote 通告成 0 的客户端能让我们的第一次
	// Write 永远阻塞在流控上——和一言不发是同一种 slowloris，只是换了一步。
	_ = st.SetWriteDeadline(deadline)

	sess, err := handshake(st, conn, cfg, r)
	if err != nil {
		// 详细错误只进服务端日志。发给对端的是粗粒度的稳定代码——
		// 这个对端还没有通过鉴权，告诉他"是签名错还是 base64 错"
		// 只会帮他调试伪造，对正当客户端没有任何用处。
		reason := reasonFor(err)
		slog.Info("handshake refused", "peer", conn.RemoteAddr().String(),
			"reason", reason, "error", err)
		// 原因走两条路，以第二条为准（见 codes.go）：BYE 是给已经在读的客户端的
		// 方便，而 CONNECTION_CLOSE 的 reason phrase 和关闭码同属一个帧、原子送达，
		// 丢不掉。一个建连之后没立刻去读控制流的客户端只拿得到后者。
		sendBye(st, reason)
		// 给对端一个读 BYE 的窗口，再关。见 byeGrace。
		select {
		case <-time.After(byeGrace.Get()):
		case <-ctx.Done():
		}
		conn.CloseWithError(CloseHandshakeRefused, reason)
		return
	}
	// 握手过了，取消读写截止时间——控制面之后是长连接，管制员可能几分钟不说话。
	_ = st.SetReadDeadline(time.Time{})
	_ = st.SetWriteDeadline(time.Time{})
	slog.Info("session opened", "session", sess.ID, "cid", sess.CID, "peer", conn.RemoteAddr().String())
	defer func() {
		r.Remove(sess.ID)
		slog.Info("session closed", "session", sess.ID, "cid", sess.CID)
	}()

	go readDatagrams(ctx, conn, r, sess.ID)
	readControl(st, r, sess)
}

// handshake 要求第一条消息必须是 HELLO，验签通过后登记会话。
func handshake(st quic.Stream, conn quic.Connection, cfg Config, r *router.Router) (*router.Session, error) {
	b, err := control.ReadFrame(st)
	if err != nil {
		return nil, err
	}
	m, err := control.Decode(b)
	if err != nil {
		return nil, err
	}
	h, ok := m.(*control.Hello)
	if !ok {
		// 什么都不能排在 HELLO 前面：一个未鉴权的连接不该能改动任何状态。
		return nil, errFirstMessageMustBeHello
	}
	claims, err := auth.Verify(cfg.PublicKey, h.Token, time.Now())
	if err != nil {
		return nil, err
	}

	sess := r.Add(router.SessionOpts{
		CID:    claims.CID,
		Follow: h.Follow,
		// MaxTX 来自 token（鉴权的一部分），MaxRX 来自服务端配置（资源上限）。
		// MaxRX 必须真的传下去：只在 READY 里通告的话那个数字就只是一句建议，
		// 一个已鉴权的会话可以声明一万个频率，每个都要在写锁里进倒排索引，
		// 全网扇出排队等它。
		MaxTX: claims.MaxTX,
		MaxRX: cfg.MaxRX,
		Send: func(p []byte) {
			// datagram 发送失败不是错误：它本来就是不可靠的，
			// 丢了就丢了，下一帧 20 毫秒后就到。
			_ = conn.SendDatagram(p)
		},
		Close: func() {
			// 同一个 CID 再次登录时，router 用这个回调把旧连接断开。
			// 不断的话，那位成员会在最长一个 MaxIdleTimeout 里有两条会话
			// 订阅同一批频率，而"不回声给自己"判的是 SessionID 不是 CID
			// ——他会听到自己的声音。
			//
			// 必须放到另一个 goroutine 上：quic-go 的 CloseWithError 最后一行是
			// `<-s.ctx.Done()`，它**等到连接的主循环真正退出**才返回。而这个回调
			// 是在 Router.Add 里、**新会话的握手路径上**同步调用的——直接调就是
			// 让新来的那个人排队等旧连接拆完。
			//
			// 由 TestTheEvictionCloseDoesNotBlockTheNewHandshake 钉住，它不靠计时
			// 碰运气——假连接把断连真的按住，所以红不红跟本机有多快无关。
			go conn.CloseWithError(CloseEvicted, "another session signed in with this account")
		},
	})
	ready := &control.Ready{
		Session: uint32(sess.ID),
		Server:  ServerVersion,
		MaxTX:   claims.MaxTX,
		MaxRX:   cfg.MaxRX,
	}
	out, err := control.Encode(ready)
	if err != nil {
		r.Remove(sess.ID)
		return nil, err
	}
	if err := control.WriteFrame(st, out); err != nil {
		r.Remove(sess.ID)
		return nil, err
	}
	return sess, nil
}

type helloError string

func (e helloError) Error() string { return string(e) }

const errFirstMessageMustBeHello = helloError("the first control message must be HELLO")

// reasonFor 把握手失败翻译成发给对端的粗粒度代码。
//
// 对端在这一刻**还没有通过鉴权**，所以内部错误原文一个字都不该给他：
// 那些文本会说出"签名长度不对"还是"payload 不是合法 JSON"，只对伪造
// token 的人有用；`token expired at 1757000000, now is 1757000100` 还会
// 顺带把服务端的时钟送出去。
//
// 但也不能一律"refused"：正当客户端需要知道"去换一张新 token 再试"
// 和"别试了"的区别，否则它只能盲目重试，而重试会撞上限速。
// 所以恰好两级，都是稳定字符串，客户端可以拿去做判断。
//
// 返回值同时是 BYE 的 reason 和 CONNECTION_CLOSE 的 reason phrase，取值见
// codes.go 的 Reason* 常量——那里也写了为什么必须以后者为准。
func reasonFor(err error) string {
	switch {
	case errors.Is(err, auth.ErrExpired):
		return ReasonTokenExpired
	case errors.Is(err, auth.ErrInvalid):
		return ReasonTokenInvalid
	default:
		return ReasonRefused
	}
}

// readControl 处理握手之后的控制面消息。
func readControl(st quic.Stream, r *router.Router, sess *router.Session) {
	for {
		b, err := control.ReadFrame(st)
		if err != nil {
			return
		}
		m, err := control.Decode(b)
		if err != nil {
			// Debug 而不是 Warn：对端可以无限发垃圾帧，Warn 会把日志刷满，
			// 而这既不是服务端的问题、也不需要运维介入。
			slog.Debug("undecodable control frame", "session", sess.ID, "error", err)
			continue
		}
		switch v := m.(type) {
		case *control.Sub:
			ack := r.Subscribe(sess.ID, *v)
			slog.Debug("subscription replaced", "session", sess.ID,
				"rx", len(ack.RX), "tx", len(ack.TX),
				"rejected", len(ack.Rejected), "rejected_xc", len(ack.RejectedXC))
			if out, err := control.Encode(&ack); err == nil {
				_ = control.WriteFrame(st, out)
			}
		case *control.Ping:
			if out, err := control.Encode(&control.Pong{T: v.T, ServerT: time.Now().UnixMilli()}); err == nil {
				_ = control.WriteFrame(st, out)
			}
		case *control.Hello:
			// 重复的 HELLO 是客户端 bug。忽略而不是重建会话——
			// 重建会换掉 session id，而那正是每个包里的 speaker 字段。
			slog.Warn("ignoring a second HELLO on an established session", "session", sess.ID)
		default:
			// 记类型而不是值：这条分支收到的是客户端本不该发的服务端侧消息，
			// 把整个结构体打进日志只会把将来某个带敏感字段的新消息一起打出去。
			slog.Warn("unexpected control message from a client",
				"session", sess.ID, "message", fmt.Sprintf("%T", m))
		}
	}
}

// readDatagrams 把上行音频交给 router 扇出。
func readDatagrams(ctx context.Context, conn quic.Connection, r *router.Router, id router.SessionID) {
	for {
		p, err := conn.ReceiveDatagram(ctx)
		if err != nil {
			return
		}
		if _, err := r.Fanout(id, p); err != nil {
			// 这里刻意是 Debug：一个还没发完 SUB 就开始说话的客户端
			// 会刷满日志，而它并不是服务端的问题。
			slog.Debug("dropped an inbound packet", "session", id, "error", err)
		}
	}
}

func sendBye(st quic.Stream, reason string) {
	if out, err := control.Encode(&control.Bye{Reason: reason}); err == nil {
		_ = control.WriteFrame(st, out)
	}
}
