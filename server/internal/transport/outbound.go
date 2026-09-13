package transport

import (
	"log/slog"
	"sync"

	"github.com/JianyueLab-Org/can-voice/server/internal/wire"
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
	if len(o.q) >= outboundDepth {
		o.dropOneLocked()
	}
	o.q = append(o.q, p)
	o.mu.Unlock()
	// 唤醒信号只需要"有活干"这一个比特：wake 满了说明排空 goroutine 还没来得及
	// 消费上一个信号，而它消费之后会一直排到队列空为止，这一帧跑不掉。
	select {
	case o.wake <- struct{}{}:
	default:
	}
}

// dropOneLocked 丢掉一帧腾位置，调用时必须持锁。
//
// 丢最老的——实时音频里迟到的帧没有价值。唯一的例外是带 FlagLast 的那一帧：
// 它是接收端用来熄灭 RX 指示灯的那一位（wire/header.go 写明它存在就是为了取代
// can-audio 那个"松开 PTT 后指示灯多亮半秒"的超时循环），丢了对方的灯就一直亮着，
// 而且没有任何东西会来纠正——等于把刚刚设计掉的那个毛病又请回来。所以从最老的
// 一端往后找第一个**不带** FlagLast 的丢掉；万一整队都是 FlagLast（病态情况），
// 才丢最老的那个。
func (o *outbound) dropOneLocked() {
	// 空队列直接回。今天到不了这里——唯一的调用方先判了 len(o.q) >= outboundDepth
	// ——但下面那句 o.q[victim+1:] 在空队列上是 o.q[1:0]，低位大于高位，会 panic。
	// 而它 panic 在排空 goroutine **之外**（enqueue 是扇出线程调的），整条会话
	// 跟着一起走。这一行是给还不存在的第二个调用方留的：比如将来某个"背压时
	// 主动排空"的助手，它没有理由知道这里有个不成文的前置条件。
	if len(o.q) == 0 {
		return
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
		slog.Info("outbound queue overflowed, dropping audio",
			"dropped_total", o.drops, "depth", outboundDepth)
	}
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
			// 发送失败不是错误：datagram 本来就不可靠，丢了就丢了，
			// 下一帧 20 毫秒后就到。连接真的关了的话，quit 会把我们叫走。
			_ = o.send(p)
		}
	}
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

func (o *outbound) dropped() uint64 {
	o.mu.Lock()
	defer o.mu.Unlock()
	return o.drops
}
