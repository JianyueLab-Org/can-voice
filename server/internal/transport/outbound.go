package transport

import (
	"errors"
	"log/slog"
	"net"
	"sync"

	"github.com/JianyueLab-Org/can-voice/server/internal/wire"
	"github.com/quic-go/quic-go"
)

// outboundDepth 是每条会话的发送队列深度，单位是帧（20 ms 一帧），
// 所以 8 帧 = 160 ms。
//
// 不是随手取的数，两端各有一条理由把它夹在这里：
//
// 下界是**一个瞬间里的突发**，不是拥塞。一轮扇出给一个听众最多投一份，但同一个
// 20 ms 里频率上可能有不止一个人在讲（两人同时按下 PTT，或者一个管制员同时监听
// 好几个频率而它们各自有人在说），于是几帧会在排空 goroutine 被调度到之前一起
// 排进来。8 帧容得下一个 tick 里 8 个同时说话的人，比现实里会发生的多得多；
// 取 2 就会在完全正常的场面下丢帧。
//
// 说清楚这条**只在排空跟得上时成立**：一旦排空真的卡住，8 帧就是总共 160 ms，
// 三个人同时讲的话每人只有约 53 ms。那不是这个数该去解决的事——排空卡住时
// 本来就该丢（见下面的上界），囤更多只是把已经没人要的音频留得更久。
//
// 上界是**延迟**。客户端的抖动缓冲一般 60–200 ms，服务端队列比它更深只是给
// 已经迟到的音频再加延迟；而 quic-go 自己还有 32 帧（640 ms）的队列压在后面
// （datagram_queue.go 的 maxDatagramSendQueueLen，v0.48.2 里没有旋钮可调），
// 我们这 8 帧是叠加上去的——一个客户端要落后约 800 ms 才开始在这里丢帧，那时候
// 这些音频早就没人要了。能控制的只有自己这一段，所以取小不取大：
// 拥塞时正确的反应是丢，不是囤。
const outboundDepth = 8

// dropReportEvery 是丢包汇报的节流间隔（按丢弃的帧数计）。
// 50 帧正好是一秒的音频。每丢一帧一行日志会把一次拥塞写成几百行；
// 一行说明"发生了什么变化"就够了。
const dropReportEvery = 50

// outbound 是一条会话的发送队列。
//
// 它存在的唯一理由：quic-go 的 SendDatagram 在自己的队列满（32 帧）时**阻塞**
// ——datagramQueue.Add 的注释原话是 "Once that limit is reached, Add blocks until
// the queue size has reduced"，而 SendDatagram 的最后一行正是 datagramQueue.Add——
// 而 router.Fanout 是串行遍历听众的。直接把 Send 接到 SendDatagram 上，一个上行
// 拥塞的客户端就会卡住整个频率的扇出，那一轮排在它后面的每个人一起哑掉。
//
// 入队永不阻塞，阻塞被关在这条会话自己的 goroutine 里。
type outbound struct {
	send func([]byte) error

	mu    sync.Mutex
	q     [][]byte
	drops uint64

	wake   chan struct{}
	quit   chan struct{}
	exited chan struct{}
	once   sync.Once

	// reportedFailure 记着这条会话已经为一次"发不出去"说过话了。
	// 只由排空 goroutine（run）读写，所以不需要锁——见 reportSendFailure。
	reportedFailure bool
}

func newOutbound(send func([]byte) error) *outbound {
	o := &outbound{
		send:   send,
		q:      make([][]byte, 0, outboundDepth),
		wake:   make(chan struct{}, 1),
		quit:   make(chan struct{}),
		exited: make(chan struct{}),
	}
	go o.run()
	return o
}

// enqueue 把一帧排进队列，**永不阻塞**。
//
// 调用方交出 p 的所有权：入队之后不得再读写它。router.Fanout 本来就给每个听众
// append 出一份新缓冲（它的注释写明了 Send 可能是异步的），所以这里不再拷贝一次
// ——每秒 50 帧乘以听众数的拷贝，省下来是值得的。
func (o *outbound) enqueue(p []byte) {
	o.mu.Lock()
	var report uint64
	if len(o.q) >= outboundDepth {
		report = o.dropOneLocked()
	}
	o.q = append(o.q, p)
	o.mu.Unlock()
	// **日志在锁外。** enqueue 跑在扇出那个 goroutine 上，它串行遍历整个频率的
	// 听众；而 slog 的默认 handler 一路写到 stderr，那是会卡的——docker 的
	// json-file 驱动、journald 背压、一个满了的磁盘，都会让一次 Write 停住。
	// 在锁里记日志等于：一个日志后端卡住 → 持锁的 enqueue 卡住 → 同一条会话的
	// 排空 goroutine 也拿不到锁 → 扇出停在这个听众身上，整个频率一起哑。
	//
	// 那正是这条有界队列**专门要消除**的那种耦合（见上面的类型注释），只是把
	// 慢的东西从"客户端的上行"换成了"日志后端"。锁里只数数，锁外才说话。
	if report > 0 {
		slog.Info("outbound queue overflowed, dropping audio",
			"dropped_total", report, "depth", outboundDepth)
	}
	// 唤醒信号只需要"有活干"这一个比特：wake 满了说明排空 goroutine 还没来得及
	// 消费上一个信号，而它消费之后会一直排到队列空为止，这一帧跑不掉。
	select {
	case o.wake <- struct{}{}:
	default:
	}
}

// dropOneLocked 丢掉一帧腾位置，调用时必须持锁。返回值是"这一次该汇报的
// 丢帧总数"，不该汇报时返回 0——记日志的事留给锁外的调用方（见 enqueue）。
//
// 用 0 当"不用汇报"的哨兵是安全的：drops 是先自增再判的，所以真要汇报的时候
// 它至少是 1。
//
// 丢最老的——实时音频里迟到的帧没有价值。唯一的例外是带 FlagLast 的那一帧：
// 它是接收端用来熄灭 RX 指示灯的那一位（wire/header.go 写明它存在就是为了取代
// can-audio 那个"松开 PTT 后指示灯多亮半秒"的超时循环），丢了对方的灯就一直亮着，
// 而且没有任何东西会来纠正——等于把刚刚设计掉的那个毛病又请回来。所以从最老的
// 一端往后找第一个**不带** FlagLast 的丢掉；万一整队都是 FlagLast（病态情况），
// 才丢最老的那个。
func (o *outbound) dropOneLocked() uint64 {
	// 空队列直接回。今天到不了这里——唯一的调用方先判了 len(o.q) >= outboundDepth
	// ——但下面那句 o.q[victim+1:] 在空队列上是 o.q[1:0]，低位大于高位，会 panic。
	// 而它 panic 在排空 goroutine **之外**（enqueue 是扇出线程调的），整条会话
	// 跟着一起走。这一行是给还不存在的第二个调用方留的：比如将来某个"背压时
	// 主动排空"的助手，它没有理由知道这里有个不成文的前置条件。
	if len(o.q) == 0 {
		return 0
	}
	// 整队都带 FlagLast 时循环走完不 break，victim 留在 0，丢最老的那个。
	victim := 0
	for i, p := range o.q {
		if !hasFlagLast(p) {
			victim = i
			break
		}
	}
	o.q = append(o.q[:victim], o.q[victim+1:]...)
	o.drops++
	if o.drops == 1 || o.drops%dropReportEvery == 0 {
		return o.drops
	}
	return 0
}

func hasFlagLast(p []byte) bool {
	return len(p) >= wire.HeaderSize && p[1]&wire.FlagLast != 0
}

func (o *outbound) pop() ([]byte, bool) {
	o.mu.Lock()
	defer o.mu.Unlock()
	if len(o.q) == 0 {
		return nil, false
	}
	p := o.q[0]
	// 置 nil 再前移：不置的话底层数组会一直握着这份已经发出去的缓冲，
	// 直到数组被整个换掉为止。深度只有 8，所以量不大，但这是一行的事。
	o.q[0] = nil
	o.q = o.q[1:]
	return p, true
}

func (o *outbound) run() {
	defer close(o.exited)
	for {
		select {
		case <-o.wake:
		case <-o.quit:
			return
		}
		for {
			p, ok := o.pop()
			if !ok {
				break
			}
			if err := o.send(p); err != nil {
				o.reportSendFailure(err, len(p))
			}
		}
	}
}

// reportSendFailure 判一次发送失败是**丢包**还是**这条会话再也发不出声**，
// 只有后者出声。
//
// 原先这里是 `_ = o.send(p)`，理由写的是"datagram 本来就不可靠，丢了就丢了"。
// 那句话对一半：quic-go v0.48.2 的 SendDatagram 恰好只会返回三种东西
// （connection.go:2276），而其中两种**不是丢包，是永久失效**——
//
//   - `*quic.DatagramTooLargeError`：这一帧比这条连接协商出来的 datagram 上限
//     还大，所以**同样大小的每一帧都会失败**，一直到连接结束。表现是这个听众
//     此后完全静默，而两端都没有一行日志。
//   - `errors.New("datagram support disabled")`：对端根本没协商 datagram 扩展。
//     那么这条会话的音频**一帧都发不出去**，从头到尾。它照样握手成功、照样在
//     台面上亮着——只是谁也听不见他、他也听不见别人。
//
// 第三种才是真的"连接没了"：datagramQueue 关闭时返回连接自己的关闭错误。它是
// 预期的（quit 马上会把我们叫走），必须保持安静，否则每一次正常断连都要多一行
// 日志。判据用 net.ErrClosed：quic-go 所有的连接关闭错误——TransportError、
// ApplicationError、IdleTimeoutError、HandshakeTimeoutError、StatelessResetError
// ——的 Is() 全都返回 `target == net.ErrClosed`（internal/qerr/errors.go），
// 所以这一条判据盖住整族，不用逐个列类型。
//
// 一条会话只说一次：50 帧/秒乘以一条永久错误，不加这个开关就是每秒 50 行。
// once 这个字段只由排空 goroutine 碰（run 是单 goroutine 的），不需要加锁。
func (o *outbound) reportSendFailure(err error, size int) {
	if errors.Is(err, net.ErrClosed) {
		// 连接关了。正常收尾路径，安静。
		slog.Debug("dropping a frame on a connection that is going away", "error", err)
		return
	}
	if o.reportedFailure {
		return
	}
	o.reportedFailure = true
	var tooLarge *quic.DatagramTooLargeError
	if errors.As(err, &tooLarge) {
		slog.Error("this connection cannot carry a voice frame this large, and it never will; the listener is silent from here on",
			"bytes", size, "max_datagram_payload", tooLarge.MaxDatagramPayloadSize, "error", err)
		return
	}
	slog.Error("voice frames cannot be sent on this connection at all; the listener hears nothing and nothing else will say so",
		"bytes", size, "error", err)
}

// stop 让排空 goroutine 退出，并返回一个在它真正退出时关闭的 channel。
//
// 它**不等**：排空 goroutine 可能正卡在 send 里（quic-go 的队列满），
// 而 stop 是从连接收尾路径上调的，不能阻塞。连接一关，那个 send 自己会返回
// （datagramQueue.Add 在 closed 上 select）。
//
// 还在队列里没发出去的帧就此作废，所以调用顺序有讲究：必须先把会话从 router
// 摘掉再 stop，见 handleConn。
func (o *outbound) stop() <-chan struct{} {
	o.once.Do(func() { close(o.quit) })
	return o.exited
}

// stopped 报告 stop 有没有被调过。
//
// 它只有一个用处：让"先摘除、再停队列"那条顺序能在**它自己那一层**被断言
// （见 closeSession）。生产代码不要拿它当控制流——排空 goroutine 自己在
// quit 上 select，不需要谁去查。
func (o *outbound) stopped() bool {
	select {
	case <-o.quit:
		return true
	default:
		return false
	}
}

func (o *outbound) dropped() uint64 {
	o.mu.Lock()
	defer o.mu.Unlock()
	return o.drops
}
