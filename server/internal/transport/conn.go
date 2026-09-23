package transport

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"sync"
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

// controlWriteTimeout 是**握手之后**每一次控制面写的时限。
//
// 它堵的是"写把读堵死"：readControl 在同一个 goroutine 里读和写，而
// initial_max_stream_data_bidi_remote 是**对端**的传输参数，quic-go v0.48.2
// 对它不设下限（internal/wire/transport_parameters.go 的
// readNumericTransportParameter 原样收下任何值）。一个把窗口调到刚好在 READY
// 之后用完、然后再也不读的对端，会让服务端的下一次写永久阻塞在流控上——于是
// 这条会话不再读任何东西：它之后的 SUB 一条都不会被处理，客户端继续收着旧的
// 那套频率，台面改动看上去毫无反应，而连接一切正常。
//
// **空闲超时救不了这件事。** MaxIdleTimeout 是 60 秒，但它在收到任何报文时
// 都会重置，而 KeepAlivePeriod=15s 的 PING 会被对端的传输层自动 ACK，跟应用
// 读不读无关。所以卡住的连接会一直活着，不是"反正 60 秒后也会断"。
//
// 取 10 秒，两边都不对称，所以往长里取：
//   - 误杀的代价是掐断一条**正常工作**的语音会话。这个网络的客户端跑在满负荷
//     的模拟器旁边、经常挂在会丢包的链路上（can-audio 那条中继存在的全部理由），
//     一次重传就能让一次合法的写走上几秒。
//   - 命中的代价只是提前关掉一条**已经悄悄坏掉**的会话。
//     10 秒大约是可信的合法停顿的六倍，又只有 MaxIdleTimeout 的六分之一，而且
//     它开火的那个区间里空闲超时被证明永远不会开火。
//
// 同样是变量只为了测试能把它调小。
var controlWriteTimeout = newDuration(10 * time.Second)

// controlReadTimeout 是**一帧已经开始到达之后**，它剩下的部分必须读完的时限。
//
// 它堵的是读侧的 slowloris，和 controlWriteTimeout 堵写侧的是同一个形状——
// conn.go 已经为写侧论证过一遍，读侧却一直没有闸。手法：写一个声称 64 KiB 的
// 长度前缀，然后每 30 秒发一个字节。control.ReadFrame 于是永远停在 io.ReadFull
// 里，这条会话和它的三个 goroutine（readControl、readDatagrams、outbound 的排空）
// 被永久占住。
//
// **QUIC 的空闲计时器救不了这件事**，理由和写侧一模一样：包**确实在到达**，
// MaxIdleTimeout 每收到一个报文就重置。连接因此一直健康，而它什么事都不做。
//
// 为什么不能给整条控制流设一个读截止时间：管制员几分钟不说话是完全正常的，
// 那种连接上**一个字节都没有**，和上面那种恰好相反。所以截止时间只在一帧的
// 第一个字节到达之后才上弦，帧读完就撤（见 framedReader）。分界线是"你已经
// 承诺了一帧"而不是"你有没有在说话"。
//
// 取 10 秒，和写侧同一个数、同一套理由：误杀一条健康会话的代价远大于多留一条
// 已经坏掉的会话十秒；而这个网络的客户端常挂在会丢包的链路上，一次重传就能让
// 一帧合法的 SUB 走上几秒。
//
// 同样是变量只为了测试能把它调小。
var controlReadTimeout = newDuration(10 * time.Second)

// deadliner 是 armHandshakeDeadlines 需要一条流做的全部事情。
//
// 收窄成两个方法而不是直接收 quic.Stream，只为**让它能被断言**：
// handshake_test.go 里那个假流实现得起这两个方法，而造一个 quic.Stream 的
// 完整替身要连读、写、关、流 id 一起实现。
type deadliner interface {
	SetReadDeadline(time.Time) error
	SetWriteDeadline(time.Time) error
}

// armHandshakeDeadlines 给控制流的读和写**同时**上截止时间；传零值就是撤掉。
//
// 一个函数而不是两行，是因为这两行必须成对：
//
//   - 上弦时只上读那一侧，就漏掉了写侧那种 slowloris——对端的传输参数是**它**
//     说了算的，一个把 initial_max_stream_data_bidi_remote 通告成 0 的客户端
//     能让服务端的第一次 Write 永远阻塞在流控上，和一言不发是同一种攻击，
//     只是换了一步。
//   - 撤销时只撤读那一侧，握手那个一次性的写时限就会**留在**一条长连接上：
//     它早晚会过期，然后掐死一条完全健康的会话，而症状发生在十秒之后、
//     和任何一次操作都对不上。
//
// 两行内联的时候没有任何东西钉得住这种成对关系（真 QUIC 连接上看不见
// 截止时间被设成了什么），所以抽成一个函数本身就是可测性那条设计反馈。
func armHandshakeDeadlines(st deadliner, t time.Time) {
	_ = st.SetReadDeadline(t)
	_ = st.SetWriteDeadline(t)
}

// framedReader 在一帧的第一个字节到达之后，给这一帧剩下的部分上读截止时间。
//
// 它必须包在 control.ReadFrame **外面**而不是里面：ReadFrame 收的是 io.Reader，
// 而截止时间是 quic.Stream 的东西，本包不该把 QUIC 的概念推进控制面协议包里。
type framedReader struct {
	st    quic.Stream
	armed bool
}

func (f *framedReader) Read(p []byte) (int, error) {
	n, err := f.st.Read(p)
	if n > 0 && !f.armed {
		// 上弦一次就够：一帧之内的后续 Read 继承同一个绝对时刻，
		// 所以"每收到一个字节就把时限往后推"那种打法拖不动它。
		f.armed = true
		_ = f.st.SetReadDeadline(time.Now().Add(controlReadTimeout.Get()))
	}
	return n, err
}

// endFrame 撤掉截止时间。一帧读完就要撤，否则下一次等待（可能是几分钟的正常
// 静默）会继承一个已经过去的时刻，每条会话都在收到第一帧之后不久自己死掉。
func (f *framedReader) endFrame() {
	if f.armed {
		f.armed = false
		_ = f.st.SetReadDeadline(time.Time{})
	}
}

// handleConn 处理一条连接的完整生命周期。
func handleConn(ctx context.Context, conn quic.Connection, cfg Config, r *router.Router, release ...func()) {
	if len(release) != 0 {
		defer func() {
			if release[0] != nil {
				release[0]()
			}
		}()
	}
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
	armHandshakeDeadlines(st, time.Now().Add(handshakeTimeout.Get()))

	// 每条会话一个有界发送队列。把 SessionOpts.Send 直接接到 conn.SendDatagram 上
	// 是不行的：那个函数在 quic-go 自己的 32 帧队列满时**阻塞**，而 router.Fanout
	// 串行遍历听众——一个上行拥塞的客户端会卡住整条频率的扇出。见 outbound.go。
	//
	// 建在这里而不是 handshake 里面，是因为收尾要用到它：握手失败和正常结束两条
	// 退出路径都必须停掉排空 goroutine，否则每条连接漏一个。
	out := newOutbound(conn.SendDatagram)

	sess, err := handshake(st, conn, cfg, r, out.enqueue)
	if err != nil {
		if len(release) != 0 && release[0] != nil {
			release[0]()
			release[0] = nil
		}
		out.stop()
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
	if len(release) != 0 && release[0] != nil {
		release[0]()
		release[0] = nil
	}
	// 握手过了，两侧的截止时间都撤掉——控制面之后是长连接，管制员可能几分钟
	// 不说话，而握手那个一次性的时限留在一条长连接上早晚会过期，然后掐死一条
	// 完全健康的会话。
	//
	// 撤掉不等于此后无人看管，两侧各自换了一套按帧计的：读是 framedReader
	// （只在一帧已经开了头之后上弦，controlReadTimeout），写是 writeControl
	// 每写一帧各设一次（controlWriteTimeout）。整条连接一个固定时刻是不行的
	// ——它要么早晚过期掐死健康会话，要么就得是无穷大，而无穷大正是那个卡死
	// 的形状。
	armHandshakeDeadlines(st, time.Time{})
	slog.Info("session opened", "session", sess.ID, "cid", sess.CID, "peer", conn.RemoteAddr().String())
	defer func() {
		closeSession(r, sess.ID, out)
		// 丢帧总数跟着会话一起收口：一次拥塞在过程中只按 dropReportEvery 汇报，
		// 这一行是那条会话的结论。
		slog.Info("session closed", "session", sess.ID, "cid", sess.CID,
			"dropped", out.dropped())
	}()

	cw := &controlWriter{st: st}
	sess.SetNotifyAuthorityLost(func() {
		b, err := control.Encode(&control.Notice{Kind: control.KindAuthorityLost})
		if err == nil {
			_ = cw.write(b)
		}
	})
	sess.SetNotifyTalker(func(speaker router.SessionID, cid string, freq uint32) {
		out, err := control.Encode(&control.Notice{
			Kind:    control.KindTalker,
			Freq:    freq,
			Session: uint32(speaker),
			CID:     cid,
		})
		if err != nil {
			return
		}
		_ = cw.write(out)
	})
	go readDatagrams(ctx, conn, r, sess, cw)
	if code, reason, ok := closeAfterControl(readControl(st, cw, r, sess)); ok {
		// 关在这里而不是靠外层那条 defer：那条发的是 CloseNormal，
		// 而 CloseNormal 的意思是"你可以重连"，在这里恰恰是错的答案。
		// 先关的那次生效（quic-go 的 closeOnce），所以 defer 变成空操作。
		slog.Info("closing a session that broke the control-stream contract",
			"session", sess.ID, "cid", sess.CID, "reason", reason, "code", uint64(code))
		conn.CloseWithError(code, reason)
	}
}

// sessionRemover 是 closeSession 需要 router 做的全部事情。
//
// 收窄成一个方法，只为**让拆除顺序能被断言**：假的 remover 可以在被调到的
// 那一刻回头问队列"你停了没有"，于是顺序变成一个可以直接判的事实，而不是
// 两行代码的相对位置。拿真的 *router.Router 是问不出这个的。
type sessionRemover interface {
	Remove(router.SessionID)
}

// closeSession 收尾一条会话：**先摘除，再停队列**。
//
// 顺序是承重的，两头都有理由：
//
// 先 Remove 再 stop——反过来的话，在摘除之前最后被扇进来的那几帧会静静地留在
// 一条已经没人排空的队列里，而它们很可能正是带 FlagLast 的尾帧。那一位是接收端
// 用来**熄灭 RX 指示灯**的（wire/header.go 写明它存在就是为了取代 can-audio 那个
// "松开 PTT 后指示灯多亮半秒"的超时循环），丢了对端的灯就一直亮着，而且没有任何
// 东西会来纠正——等于把刚刚设计掉的那个毛病又请回来。
//
// stop 之后**不等** exited：排空 goroutine 这时可能正卡在 SendDatagram 里，而把
// 它叫醒的是 handleConn 外层那条 `defer conn.CloseWithError`——它排在这条 defer
// 后面。等在这里就是死锁。
//
// 单独一个函数而不是两行内联：这两行互换在此之前**整套测试照绿**，而注释早就
// 预言了症状。见 TestTheSessionIsUnregisteredBeforeItsQueueStops。
func closeSession(r sessionRemover, id router.SessionID, out *outbound) {
	r.Remove(id)
	out.stop()
}

// closeAfterControl 把 readControl 的返回值翻成关闭码和原因串。
//
// 单独一个纯函数，是因为它**在别处钉不住**：走到这里的三种死因各有各的难走——
// 一种要造一个会卡住流控的对端，一种要一帧永远发不完，第三种在声明有了上界之后
// 根本不可达。而这张表本身是协议的一部分——码和原因串是客户端写死在自己代码里
// 的字面值。
//
// 第三个返回值是"要不要主动关"，而不是拿 code == 0 当哨兵：CloseNormal 就是 0，
// 那样就没法表达"什么都不用做，走外层那条正常收尾"。
func closeAfterControl(err error) (quic.ApplicationErrorCode, string, bool) {
	switch {
	case errors.Is(err, errControlWriteStalled):
		return CloseProtocolViolation, ReasonControlWriteStalled, true
	case errors.Is(err, errControlReadStalled):
		return CloseProtocolViolation, ReasonControlReadStalled, true
	case errors.Is(err, errAckUndeliverable):
		return CloseProtocolViolation, ReasonAckUndeliverable, true
	default:
		// nil（正常读到头、对端挂断）以及别的写错误都走外层的 CloseNormal：
		// 那些情况下"你可以重连"是对的答案。
		return 0, "", false
	}
}

// handshake 要求第一条消息必须是 HELLO，验签通过后登记会话。
//
// send 是下行数据面的入口，由调用方给（生产路径上是 outbound 队列的 enqueue）。
// 刻意不在这里自己去接 conn.SendDatagram：那样队列就只有 handshake 拿得到，
// 而停掉它是 handleConn 的收尾职责。
func handshake(st quic.Stream, conn quic.Connection, cfg Config, r *router.Router, send func([]byte)) (*router.Session, error) {
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
	// 版本要真的看。README 和 codes.go 都已经把"协议版本不合"写成关闭码 1 的
	// 一条成因，而这个字段此前从没被读过：`"proto": 99999` 照样拿到 READY。
	//
	// 判在验签**之前**：它比一次 Ed25519 验签便宜，而且优先级是对的——一个版本
	// 对不上的客户端，就算票是新的也一样连不通，告诉他"去换票"是把他送错方向。
	if h.Proto != control.ProtoVersion {
		return nil, errProtoUnsupported
	}
	// Follow 是观察员跟随的呼号，而它会**被当成 map 的键**，每帧每听众查一次
	// （router 的 lookup）。无校验的话一个 6 万字节的值就那么进去了。
	//
	// 判在验签之前，理由同上：这是一条协议格式错误，和这张票新不新无关。
	if h.Follow != "" {
		return nil, errFollowNotACallsign
	}
	// Station 同样进 map 的键（router 的顶号表），同样要先校形状。
	// 它装的是席位呼号（`ZSPD_ATIS`），和 Follow 一个形状，所以用同一条规则。
	if h.Station != "" && !isValidCallsign(h.Station) {
		return nil, errStationNotACallsign
	}
	claims, err := auth.Verify(cfg.PublicKey, h.Token, time.Now())
	if err != nil {
		return nil, err
	}
	if claims.Callsign != "" && !isValidCallsign(claims.Callsign) {
		return nil, helloError("signed controller callsign is invalid")
	}
	if claims.Station != "" && !isValidCallsign(claims.Station) {
		return nil, helloError("signed ATIS station is invalid")
	}
	if claims.Role == "atis" {
		if h.Station != claims.Station {
			return nil, helloError("HELLO station differs from signed ATIS station")
		}
	} else if h.Station != "" {
		return nil, helloError("ordinary sessions cannot declare a station")
	}
	// 未定级的成员不能用语音。这不是新加的策略，是**把已有的那道闸补回来**：
	// can-api 的 /api/v1/public/auth——这个网络上其它每一个组件用的那道凭据
	// 检查——明文拒绝 rating < 1，can-audio 的文档也写着"未定级成员即便凭据
	// 正确也不能用语音"。签名验过只说明这张票是 can-api 签的，不说明持票人
	// 够格；少了这一条，can-voice 就是全网唯一一个放未定级成员进来的入口，
	// 也就是一次针对被它替换掉的那套系统的准入回退。
	//
	// 判在这里而不是 auth.Verify 里：Verify 回答的是"这张票是不是真的、有没有
	// 过期"，rating 是准入策略。两者混在一起的话，一个未定级成员会收到
	// token_invalid，然后照着那个指示反复去换新票。
	if claims.Rating < minRating {
		return nil, errRatingTooLow
	}

	// MaxTX 是**唯一**一个直接来自对端的资源上限，所以它也要被服务端夹一次。
	maxTX := grantedMaxTX(claims.MaxTX, cfg.MaxRX)
	if (claims.Role == "pilot" || claims.Role == "observer") && maxTX > 1 {
		maxTX = 1
	}
	role := claims.Role
	if role == "" {
		if !cfg.unsafeLegacyTXForTests {
			role = "legacy"
			maxTX = 0
		}
	}
	grant := make([]uint32, len(claims.TX))
	for i, freq := range claims.TX {
		grant[i] = uint32(freq)
	}

	sess := r.Add(router.SessionOpts{
		CID:          claims.CID,
		Role:         role,
		Callsign:     claims.Callsign,
		TXGrant:      grant,
		GrantExpires: time.Unix(claims.Exp, 0),
		// Station 把顶号的键从 CID 变成 (CID, station)。不传下去的话通播
		// 机队还是整队互踢——字段收下了、校验过了、然后丢掉，是最难查的那种。
		Station: claims.Station,
		// MaxTX 来自 token（鉴权的一部分），MaxRX 来自服务端配置（资源上限）。
		// MaxRX 必须真的传下去：只在 READY 里通告的话那个数字就只是一句建议，
		// 一个已鉴权的会话可以声明一万个频率，每个都要在写锁里进倒排索引，
		// 全网扇出排队等它。
		MaxTX: maxTX,
		MaxRX: cfg.MaxRX,
		// 入队，不是直接发。所有权在这里移交：Fanout 已经给每个听众 append 出
		// 一份新缓冲，队列不再拷贝一次，调用方交出去之后不得再碰它。
		Send: send,
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
			go conn.CloseWithError(CloseEvicted, ReasonEvicted)
		},
	})
	ready := &control.Ready{
		Session: uint32(sess.ID),
		Server:  ServerVersion,
		// 回显的必须是**夹过之后**的值。回显 token 原样的那个数字等于对客户端
		// 撒谎：它照着灰不掉多余的 TX 开关，按下去之后只会拿到一条静默的拒绝。
		MaxTX: maxTX,
		MaxRX: cfg.MaxRX,
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

// errProtoUnsupported 是"HELLO 里的 proto 不是本服务端讲的那个版本"。
//
// 它有自己的原因串（ReasonProtoUnsupported），不走 default 那个 ReasonRefused。
// 两者的**动作**一样——都别原样重试，换多少张票都一样——但对人说的话不一样：
// 这个网络的客户端是装在成员机器上的桌面程序，版本太旧的用户该看到"请更新
// 客户端"，而"被拒绝"会把他送去查密码。详见 codes.go。
const errProtoUnsupported = helloError("the client declared a control-plane protocol version this server does not speak")

// errFollowNotACallsign 是"观察员跟随的那个呼号不是一个呼号"。
const errFollowNotACallsign = helloError("the follow field is not a valid callsign")

// errStationNotACallsign 是"席位标记不是一个呼号"。
const errStationNotACallsign = helloError("the station field is not a valid callsign")

// 呼号的形状，照抄 can-fsd 的 IsValidCallsign（internal/fsd/packet.go）：
// 2–10 个字符，只许 A-Z、0-9、`-`、`_`。
//
// 必须照抄而不是自己定一套：Follow 的用途就是去 fsdfeed 的快照里按呼号查位置，
// 而那份快照里的呼号全都是 can-fsd 收下的，也就是全都满足这条规则。比它松的
// 规则只会放进一批**永远查不到**的键；比它紧的规则会把合法呼号挡在外面，
// 表现是那位观察员的射程判定退回"位置未知"，安静地全放行。
//
// 保留呼号（can-fsd 那边还查一张 reservedCallsigns 表）不抄：那张表管的是
// "谁能以这个身份登录 FSD"，而这里只是一个查表的键，多挡一个名字没有意义。
const (
	minCallsignLen = 2
	maxCallsignLen = 10
)

func isValidCallsign(s string) bool {
	if len(s) < minCallsignLen || len(s) > maxCallsignLen {
		return false
	}
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch {
		case c >= '0' && c <= '9', c >= 'A' && c <= 'Z', c == '-', c == '_':
		default:
			return false
		}
	}
	return true
}

// minRating 是能用语音的最低等级。
//
// 1 而不是 0，因为 can-api 的 /api/v1/public/auth 就是这么判的（`rating < 1`
// 拒绝），而那是这个网络上每一个组件共用的凭据检查。这个数字必须跟着它走，
// 不是本服务端自己的政策。
const minRating = 1

// serverMaxTX 是服务端对"同时发射几个频率"的硬上限，不管 token 里写了什么。
//
// 为什么已鉴权的字段还要夹：`session.go` 自己写着"**已鉴权不等于可信**"，而
// MaxTX 是唯一一个没有被这句话约束住的已鉴权字段——MaxRX 是服务端配置，
// CID/Rating 是身份，只有 MaxTX 是对端带进来的一个**资源上限**。一张
// `max_tx: 100000` 的 token（签发方一个笔误，或者私钥出了事）能让一条会话在
// 十万个频率上登记，而每一个都要进写锁里的倒排索引。
//
// 16 的来历：这是"一个管制员同时发射的频率数"的上界，不是订阅数。真实的
// 无线电台面（can-audio 的 controller 那一套、TrackAudio）是个位数；16 已经
// 是它的两三倍，够宽到不会误伤任何真实席位。
const serverMaxTX = 16

// grantedMaxTX 是 token 声明的 max_tx、服务端硬上限和 MaxRX 三者取小。
//
// 为什么要再夹一次 MaxRX：TX ⊆ RX，TX 频率会先写进 next.rx，而且**不受 MaxRX
// 闸门挤压**（router.Subscribe 里那个 `already` 判断，是有意的设计）。于是
// 一条会话真正能进倒排索引的频率数是 max(MaxTX, MaxRX)，MaxTX 比 MaxRX 大多少
// 就绕过多少——一张 max_tx=10000 的 token 在 MaxRX=32 的服务器上照样能登记
// 一万个频率。夹到 MaxRX 之后这个数就真的是 MaxRX 了。
func grantedMaxTX(claimed, maxRX int) int {
	return min(claimed, serverMaxTX, maxRX)
}

// errRatingTooLow 走 reasonFor 的 default 分支，也就是 ReasonRefused。
//
// 刻意不给它一个自己的原因串：对端拿到的两级代码分的是"去换一张新 token 再试"
// 和"别试了"，而一个未定级的成员换多少张票都一样——换票不会给他升级。他该做的
// 是去考试，那是服务端说不出口的事，只能落在"refused"。真正的原因在服务端日志里。
const errRatingTooLow = helloError("the member's rating is below the minimum for voice")

// reasonFor 把握手失败翻译成发给对端的粗粒度代码。
//
// 对端在这一刻**还没有通过鉴权**，所以内部错误原文一个字都不该给他：
// 那些文本会说出"签名长度不对"还是"payload 不是合法 JSON"，只对伪造
// token 的人有用；`token expired at 1757000000, now is 1757000100` 还会
// 顺带把服务端的时钟送出去。
//
// 但也不能一律"refused"：正当客户端需要知道"去换一张新 token 再试"
// 和"别试了"的区别，否则它只能盲目重试，而重试会撞上限速。
//
// 恰好三级，都是稳定字符串，客户端可以拿去做判断：**去换票**（token_expired）、
// **去更新客户端**（proto_unsupported）、**别试了**（token_invalid / refused）。
// 第二级的粒度不违反上面那条保密理由：proto 是客户端**自己声明**的字段，
// 把它说回去没有泄露任何东西，而它对应的人类动作和另外两级都不一样。
//
// 返回值同时是 BYE 的 reason 和 CONNECTION_CLOSE 的 reason phrase，取值见
// codes.go 的 Reason* 常量——那里也写了为什么必须以后者为准。
func reasonFor(err error) string {
	switch {
	case errors.Is(err, auth.ErrExpired):
		return ReasonTokenExpired
	case errors.Is(err, auth.ErrInvalid):
		return ReasonTokenInvalid
	case errors.Is(err, errProtoUnsupported):
		// 第三级，不是粗粒度的例外：动作和 refused 一样，但这一条要让客户端
		// 说得出"请更新"。见 ReasonProtoUnsupported。
		return ReasonProtoUnsupported
	default:
		return ReasonRefused
	}
}

// noticeBudget 是每条会话为"解不开的帧"回 NOTICE 的条数上限，之后转为静默。
//
// 有上限的理由不是日志噪音，是**写会阻塞读**：回 NOTICE 走的是控制流，而
// readControl 在同一个 goroutine 里读和写，所以一个不读的对端能用流控把这条
// 会话的读一起顶死（详见 controlWriteTimeout）。
//
// 说清楚这个上限**不是**那件事的完整答案，它只封掉最便宜的那条路：一帧 12 字节
// 的 `{"type":"x"}` 换服务端一条回复，SUBACK/PONG 都没有这么便宜（要先构造
// 一条合法的 SUB 或 PING）。真正兜底的是 controlWriteTimeout——因为
// initial_max_stream_data_bidi_remote 是对端说了算的，一个把窗口调到刚好在 READY
// 之后用完的对端，会让**握手后的第一次写**就卡住，什么预算都拦不住它。
const noticeBudget = 8

// readControl 处理握手之后的控制面消息。
//
// 返回 errControlWriteStalled 表示对端不再读控制流了（见 controlWriteTimeout）；
// 其余情况返回 nil——读到头、对端挂断、连接已经没了，都走正常收尾。
func readControl(st quic.Stream, cw *controlWriter, r *router.Router, sess *router.Session) error {
	// 每条会话一份预算，随连接一起消失。
	notices := noticeBudget
	fr := &framedReader{st: st}
	for {
		b, err := control.ReadFrame(fr)
		if err != nil {
			// 只有**帧读到一半**时的超时算协议违规：一个字节都没来的时候那种
			// 静默是正常的（一个只监听、不讲话的管制员一坐就是几分钟）。
			//
			// fr.armed 这半个条件今天是**纵深防御**，不是做出这个判定的依据。
			// 结构上它推不出任何输入：读截止时间只在 framedReader 上弦时被设上，
			// endFrame() 和握手之后的 armHandshakeDeadlines(st, time.Time{}) 都会
			// 把它撤掉，所以"没上弦却冒出 os.ErrDeadlineExceeded"没有任何输入
			// 造得出来——去掉这半个条件整套测试照绿（终审 L-8 实测）。留着它是为了
			// 将来：谁哪天给整条流加一个与帧无关的读截止时间，它就是第二道闸。
			if fr.armed && errors.Is(err, os.ErrDeadlineExceeded) {
				slog.Info("a peer started a control frame and then stopped sending it",
					"session", sess.ID, "cid", sess.CID,
					"timeout", controlReadTimeout.Get().String())
				return errControlReadStalled
			}
			return nil
		}
		fr.endFrame()
		m, err := control.Decode(b)
		if err != nil {
			// Debug 而不是 Warn：对端可以无限发垃圾帧，Warn 会把日志刷满，
			// 而这既不是服务端的问题、也不需要运维介入。
			slog.Debug("undecodable control frame", "session", sess.ID, "error", err)
			// 但要告诉对端一声。不断开（见 control.KindUnknownMessage），也不静默：
			// 发出去了、什么都没发生、还不知道为什么，是这个项目在别处明确拒绝的
			// 失败形态。
			if notices > 0 {
				notices--
				if err := sendUnknownNotice(cw, b); err != nil {
					return err
				}
				if notices == 0 {
					slog.Info("this session has spent its notice budget for undecodable frames, going quiet",
						"session", sess.ID, "budget", noticeBudget)
				}
			}
			continue
		}
		switch v := m.(type) {
		case *control.Sub:
			ack := r.Subscribe(sess.ID, *v)
			slog.Debug("subscription replaced", "session", sess.ID,
				"rx", len(ack.RX), "tx", len(ack.TX),
				"rejected", len(ack.Rejected), "rejected_xc", len(ack.RejectedXC))
			out, err := control.Encode(&ack)
			if err != nil {
				// 这条 SUB **已经生效了**，而它的回执发不出去。以前这里是
				// `if err == nil` 一笔带过，于是走到外层那条 defer 的
				// CloseNormal——实测的样子是 `Application error 0x0`、reason 为空、
				// 日志只有一行 `session closed dropped=0`。而码 0 的意思是
				// "你可以重连"，客户端于是重连、重放同一份 SUB，再一次静默失败：
				// 两端都没有一个字说出发生了什么的死循环。
				slog.Error("cannot encode the SUBACK for a subscription that already took effect",
					"session", sess.ID, "cid", sess.CID,
					"rx", len(ack.RX), "tx", len(ack.TX),
					"rejected", len(ack.Rejected), "rejected_xc", len(ack.RejectedXC),
					"error", err)
				return errAckUndeliverable
			}
			if err := cw.write(out); err != nil {
				if len(out) > control.MaxFrame {
					// 同上，另一半：编得出来但超过帧上限。声明有了上界之后这条路应当
					// 不可达（声明本身有上界，maxRejected 和 maxXCPairs 又各自
					// 封住了回报），**但防线要留着，而且要出声**——不可达是一个
					// 会被下一次改动悄悄推翻的结论。
					slog.Error("the SUBACK for a subscription that already took effect is too large to send",
						"session", sess.ID, "cid", sess.CID, "bytes", len(out),
						"limit", control.MaxFrame, "error", err)
					return errAckUndeliverable
				}
				return err
			}
		case *control.Ping:
			if out, err := control.Encode(&control.Pong{T: v.T, ServerT: time.Now().UnixMilli()}); err == nil {
				if err := cw.write(out); err != nil {
					return err
				}
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

// controlWriter 串行化控制流的写入。
//
// 在 tx_denied 之前控制流只有一个写者——readControl 那条 goroutine——所以不需要
// 它。而 tx_denied 要从 readDatagrams 发出去，于是有了第二个写者，
// 而 control.WriteFrame 是"长度前缀 + 载荷"两次 Write：两个 goroutine 交错写会把
// 这条流写成乱码，症状是对端解析失败、断开、重连，没有一处指回这里。
type controlWriter struct {
	mu sync.Mutex
	st quic.Stream
}

func (w *controlWriter) write(b []byte) error {
	w.mu.Lock()
	defer w.mu.Unlock()
	return writeControl(w.st, b)
}

// txDeniedEvery 是同一个频率上两条 tx_denied 之间的最小间隔。
//
// 按住 PTT 的客户端每秒发 50 个包，一包一条 NOTICE 就是一场针对控制流的拒绝
// 服务——而同一条流上还跑着 SUBACK 和 PONG。**第一条不等**：按下去要立刻知道。
const txDeniedEvery = 3 * time.Second

// txDeniedFreqs 是记账表的上界。
//
// 频率是对端随便填的 32 位数（服务端不做范围校验，见 wire.Header），
// 不封顶的话一个乱发的客户端能让这张表一直长下去。
const txDeniedFreqs = 64

// rateLimitNoticeEvery 是同一条会话两条 rate_limited 之间的最小间隔。
//
// 和 txDeniedEvery 同一个理由、同一个值：触发它的是一路每秒几百上千帧的上行，
// 一帧一条 NOTICE 就是拿一个静默失败换一场针对控制流的拒绝服务，而同一条流上还
// 跑着 SUBACK 和 PONG。**第一条不等**：出故障的那一刻要立刻说。
//
// 刻意不用 noticeBudget 那种"发够几条就永远闭嘴"的形状。那个预算配的是"解不开的
// 帧"——对端的协议版本不会在一条连接里变回来，所以说过就不必再说。超速是会好
// 的：桶自己回血，客户端也可能只是抖了一下。用完就闭嘴的话，同一条会话第二次
// 出故障时服务端一个字都不会说，而那正是它最需要说话的时候。
const rateLimitNoticeEvery = 3 * time.Second

// readDatagrams 把上行音频交给 router 扇出。
func readDatagrams(ctx context.Context, conn quic.Connection, r *router.Router, sess *router.Session, cw *controlWriter) {
	id := sess.ID
	// 这几样只有这一条 goroutine 碰，所以都不用锁。
	denied := make(map[uint32]time.Time)
	toldDegraded := false
	// 限速的状态同上。桶按这条会话**授权后**的 MaxTX 算额度，见 uplink.go。
	limiter := newUplinkLimiter(sess.MaxTX, time.Now())
	var limitedAt time.Time
	var limitedDrops uint64
	for {
		p, err := conn.ReceiveDatagram(ctx)
		if err != nil {
			return
		}
		// **限速排在最前面，连包头都不解。** 位置是有意的：正常路径上它只多了
		// 一次 time.Now() 和几次浮点运算，而超额路径上它省掉的是 wire.Parse、
		// router 的读锁和一整轮听众遍历——一路失控的上行正是靠那一整轮把同频率上
		// 每个听众队列里的正常语音挤出去的（outbound.go 的队列满了丢最旧的）。
		now := time.Now()
		if !limiter.allow(now) {
			limitedDrops++
			// 零值的 limitedAt 减出来是一段极大的时长，所以第一条立刻就发。
			if now.Sub(limitedAt) >= rateLimitNoticeEvery {
				limitedAt = now
				// Info 而不是 Debug：这条是要在排障时找得到的。节流之后一次
				// 故障只留几行，不会刷屏。
				slog.Info("uplink audio is over the rate limit, dropping frames",
					"session", id, "cid", sess.CID, "dropped", limitedDrops,
					"max_tx", sess.MaxTX)
				sendRateLimited(cw)
			}
			continue
		}
		if _, err := r.Fanout(id, p); err != nil {
			// 这里刻意是 Debug：一个还没发完 SUB 就开始说话的客户端
			// 会刷满日志，而它并不是服务端的问题。
			slog.Debug("dropped an inbound packet", "session", id, "error", err)
			var td *router.TxDeniedError
			if errors.As(err, &td) {
				sendTxDenied(cw, td.FreqKHz, denied)
			}
			continue
		}
		// 降级一次说一句，不是一帧说一句——它会持续几分钟到几小时，
		// 而上行是每秒五十帧。恢复之后重新上弦，第二次降级还会说。
		//
		// **只告诉正在发言的人。** 纯听众也受影响（他听得见全世界），但控制面
		// 今天没有服务端主动广播的路子，而为这一条消息造一个不值得：
		// 语音传到不该传到的地方，才是那个要当场知道的方向。
		degraded := r.PositionsDegraded()
		if degraded && !toldDegraded {
			sendRangeUnavailable(cw)
		}
		toldDegraded = degraded
	}
}

// sendRateLimited 回一条"你的上行超速了，多出来的被丢掉了"。
//
// 不带频率：桶是按会话算的，一次超额不归某一个频率（见 control.KindRateLimited）。
// 写失败不处理，理由同 sendTxDenied——控制流坏了的话 readControl 那条会先撞上。
func sendRateLimited(cw *controlWriter) {
	out, err := control.Encode(&control.Notice{
		Kind:   control.KindRateLimited,
		Reason: "uplink audio is above the rate a 20 ms Opus stream can produce; the excess is being dropped",
	})
	if err != nil {
		return
	}
	_ = cw.write(out)
}

// sendRangeUnavailable 回一条"射程过滤现在不生效"。
func sendRangeUnavailable(cw *controlWriter) {
	out, err := control.Encode(&control.Notice{
		Kind:   control.KindRangeUnavailable,
		Reason: "the position snapshot is unavailable; every frequency is global until it returns",
	})
	if err != nil {
		return
	}
	_ = cw.write(out)
}

// sendTxDenied 回一条"你没有在这个频率上声明发射"，按频率节流。
//
// 不发的话，一个在没声明 TX 的频率上按住 PTT 的人，界面、日志、对端三处都
// 看不出任何异常——而客户端那边 pump.rs 的接收分支早就写好了，一直是死代码。
func sendTxDenied(cw *controlWriter, freq uint32, last map[uint32]time.Time) {
	now := time.Now()
	if t, ok := last[freq]; ok && now.Sub(t) < txDeniedEvery {
		return
	}
	if len(last) >= txDeniedFreqs {
		clear(last)
	}
	last[freq] = now
	out, err := control.Encode(&control.Notice{
		Kind:   control.KindTxDenied,
		Freq:   freq,
		Reason: "transmit was not declared on this frequency",
	})
	if err != nil {
		return
	}
	// 写失败在这里不处理：控制流坏了的话 readControl 那条会先撞上并关掉连接，
	// 而这条 goroutine 没有权力替它做那个决定。
	_ = cw.write(out)
}

// sendUnknownNotice 回一条"这一帧我解不开"。
//
// Reason 里放的是**类型字符串**，不是原始报文：报文可能有几十 KB，而且把对端
// 发来的字节原样回显没有任何用处。连 JSON 都不是的帧没有类型可回，用一个固定的
// 记号代替——那个位置绝不能留空，"服务端说不出是哪里不对"和静默一样难查。
//
// 这不是放大攻击面：QUIC 面向连接且验证过地址，NOTICE 只回到同一条**已鉴权**的
// 连接上，到不了第三方那里。
func sendUnknownNotice(cw *controlWriter, frame []byte) error {
	reason := control.TypeOf(frame)
	if reason == "" {
		reason = "unparseable"
	}
	out, err := control.Encode(&control.Notice{
		Kind:   control.KindUnknownMessage,
		Reason: reason,
	})
	if err != nil {
		return nil
	}
	return cw.write(out)
}

// errControlWriteStalled 是"对端不再读控制流了"。
//
// 单独一个哨兵而不是把 os.ErrDeadlineExceeded 一路传上去：调用方要判的是
// "这是不是一次协议违规"，而截止时间超时在别的地方也可能冒出来，含义并不相同。
var errControlWriteStalled = errors.New("the peer stopped reading the control stream")

// errControlReadStalled 是"对端开了一帧然后不发完了"（见 controlReadTimeout）。
var errControlReadStalled = errors.New("the peer started a control frame and stopped sending it")

// errAckUndeliverable 是"这条 SUB 已经生效，但它的 SUBACK 发不出去"。
//
// 必须是一条**单独**的死因，不能悄悄走正常收尾：客户端的订阅状态和服务端的
// 已经对不上了，而它不知道。以 CloseNormal（"你可以重连"）收场的话，它会重连、
// 重放同一份声明，再一次得到同样的静默——一个两端都没有日志的死循环。
var errAckUndeliverable = errors.New("the SUBACK for an applied subscription cannot be sent")

// writeControl 写一帧控制面消息，带写截止时间。
//
// 每帧各自设一次而不是整条连接设一个：一个固定的绝对时刻要么早晚过期掐死一条
// 健康会话，要么就得是无穷大。理由和取值见 controlWriteTimeout。
//
// 超时不重试，往上报给调用方去关连接：control.WriteFrame 是长度前缀加载荷两次
// Write，超时可能正好落在两次之间，这条流已经不同步，没有可以续下去的东西。
func writeControl(st quic.Stream, b []byte) error {
	_ = st.SetWriteDeadline(time.Now().Add(controlWriteTimeout.Get()))
	err := control.WriteFrame(st, b)
	// 写完就清掉：留着一个过去的时刻会让下一次写立刻失败，留着一个将来的时刻
	// 会让下一帧继承一段被吃掉的预算。下一次写自己会重设。
	_ = st.SetWriteDeadline(time.Time{})
	if err != nil && errors.Is(err, os.ErrDeadlineExceeded) {
		// quic-go 的 deadlineError.Unwrap() 返回的正是这个哨兵（stream.go:21）。
		return errControlWriteStalled
	}
	return err
}

func sendBye(st quic.Stream, reason string) {
	if out, err := control.Encode(&control.Bye{Reason: reason}); err == nil {
		_ = control.WriteFrame(st, out)
	}
}
