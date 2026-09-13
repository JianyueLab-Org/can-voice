package transport

import (
	"sync"
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/wire"
)

// 本文件钉住每会话的有界发送队列。
//
// 它存在的理由在 outbound.go 的注释里：quic-go v0.48.2 的 SendDatagram 最后一行是
// datagramQueue.Add，而那个函数在自己的 32 帧队列满时**阻塞**（它自己的注释就是
// 这么写的）；router.Fanout 又是串行遍历听众的。两者相乘 = 一个上行拥塞的客户端
// 把整条频率的扇出按在它身上。
//
// 这里的测试全部靠"把排空 goroutine 按在 send 里"来取得确定性，不靠 sleep 猜时序——
// 见 parkOutbound。

// parkSeq 和 lastSeq 是两个刻意远离 0..2*outboundDepth 的 seq，
// 这样断言里认错一帧就是认错，不会撞上循环里造的那批。
const (
	parkSeq  uint16 = 900
	lastSeq  uint16 = 500
	waitStep        = time.Millisecond
	waitMax         = 3 * time.Second
)

// parked 是一条排空 goroutine 已经被按住的队列。
type parked struct {
	o       *outbound
	release chan struct{}
	freed   sync.Once

	mu   sync.Mutex
	sent [][]byte
}

// parkOutbound 造一条排空 goroutine 已经停在 send 里的队列。
//
// 这是本文件里每条丢弃策略测试的地基，理由是**确定性**：newOutbound 一建好，
// 排空 goroutine 就开始抢队头，它抢走第几帧完全取决于调度——直接灌帧再断言
// 队列里剩下谁，等于掷骰子。所以先单独入队一帧，等它确认进了 send 并被按住；
// 此后排空 goroutine 一步都动不了，队列的内容完全由测试自己摆布。
func parkOutbound(t *testing.T) *parked {
	t.Helper()
	p := &parked{release: make(chan struct{})}
	entered := make(chan struct{}, 1)
	p.o = newOutbound(func(b []byte) error {
		p.mu.Lock()
		p.sent = append(p.sent, b)
		p.mu.Unlock()
		select {
		case entered <- struct{}{}:
		default:
		}
		<-p.release
		return nil
	})
	t.Cleanup(func() {
		p.o.stop()
		p.free()
	})

	p.enqueue(t, frame(parkSeq, 0))
	select {
	case <-entered:
	case <-time.After(waitMax):
		t.Fatal("the drain goroutine never reached send; every assertion below about what sits in the queue would be racing it")
	}
	return p
}

// free 放行被按住的 send。可以重复调。
func (p *parked) free() { p.freed.Do(func() { close(p.release) }) }

// enqueue 入队一帧，并且**带上界地**等它返回。
//
// enqueue 的契约就是永不阻塞，所以直接调本来也对。用这个包装是为了红的形状：
// 把 enqueue 改回同步调用的回归会让这里永远卡住，于是每一条用 parkOutbound 的
// 测试都只能以 `panic: test timed out` 的方式红——而那句话对下一个人什么都不说。
// 和 conn_test.go 的 replyWait 是同一回事。
//
// （指定的那条钉子 TestASlowListenerDoesNotStallTheFanout 不经过这里，它自己
// 有干净的 2 秒断言；这里管的是其余四条的连带伤害。）
func (p *parked) enqueue(t *testing.T, b []byte) {
	t.Helper()
	done := make(chan struct{})
	go func() { defer close(done); p.o.enqueue(b) }()
	select {
	case <-done:
	case <-time.After(waitMax):
		t.Fatal("enqueue did not return; it must never block, and a queue whose drain goroutine is parked in send is exactly where a synchronous regression shows up")
	}
}

// letGo 放行排空 goroutine，等到它至少发出 want 帧，返回发出去的全部帧。
func (p *parked) letGo(t *testing.T, want int) [][]byte {
	t.Helper()
	p.free()
	deadline := time.Now().Add(waitMax)
	for {
		p.mu.Lock()
		n := len(p.sent)
		p.mu.Unlock()
		if n >= want {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("the queue delivered %d frames, want at least %d", n, want)
		}
		time.Sleep(waitStep)
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([][]byte(nil), p.sent...)
}

// TestASlowListenerDoesNotStallTheFanout 是这个文件存在的理由。
//
// 这是一条否定式断言（"快的那个没有被慢的那个拖住"），所以它自带前提的证明：
// 先确认快的那条**确实投递了**，"没被拖住"才是一句有内容的话。
func TestASlowListenerDoesNotStallTheFanout(t *testing.T) {
	// 慢的那一条：send 永远不返回，模拟 quic-go 的 Add 在队列满时阻塞。
	stuck := make(chan struct{})
	t.Cleanup(func() { close(stuck) })
	slow := newOutbound(func([]byte) error { <-stuck; return nil })
	defer slow.stop()

	// 快的那一条：如实记录收到了什么。
	var mu sync.Mutex
	var got [][]byte
	delivered := make(chan struct{}, 1)
	fast := newOutbound(func(p []byte) error {
		mu.Lock()
		got = append(got, p)
		mu.Unlock()
		select {
		case delivered <- struct{}{}:
		default:
		}
		return nil
	})
	defer fast.stop()

	// 模拟一轮扇出：串行地投给两个听众，慢的排在前面。
	done := make(chan struct{})
	go func() {
		defer close(done)
		for i := 0; i < outboundDepth*3; i++ {
			slow.enqueue(frame(uint16(i), 0))
			fast.enqueue(frame(uint16(i), 0))
		}
	}()

	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("the fan-out is still blocked on the slow listener; enqueue must never wait")
	}

	// 前提的证明：快的那条确实投递了。没有这一句，上面那条断言在
	// "两条都什么也没做"的情况下也会通过。
	select {
	case <-delivered:
	case <-time.After(2 * time.Second):
		t.Fatal("the fast listener received nothing; the premise of this test is gone")
	}
}

// TestEnqueueDoesNotBlockWhenTheQueueIsFull 钉住入队在任何情况下都不阻塞。
// 用一个永远不返回的 send，灌满队列再多灌几帧，整个过程必须在毫秒级完成。
func TestEnqueueDoesNotBlockWhenTheQueueIsFull(t *testing.T) {
	stuck := make(chan struct{})
	t.Cleanup(func() { close(stuck) })
	o := newOutbound(func([]byte) error { <-stuck; return nil })
	defer o.stop()

	const rounds = outboundDepth * 4
	done := make(chan time.Duration, 1)
	go func() {
		start := time.Now()
		for i := 0; i < rounds; i++ {
			o.enqueue(frame(uint16(i), 0))
		}
		done <- time.Since(start)
	}()

	select {
	case d := <-done:
		if d > time.Second {
			t.Fatalf("%d enqueues took %v against a send that never returns; enqueue is waiting on something", rounds, d)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("enqueue is still blocked with a full queue and a send that never returns; a whole frequency's fan-out would be stuck right here")
	}

	// 前提的证明：队列确实溢出了。没有这一句，"没阻塞"在
	// "send 其实一直在把队列排空"的情况下也会通过。
	if o.dropped() == 0 {
		t.Fatal("nothing was dropped, so the queue never actually filled; this test proved nothing")
	}
}

// TestOverflowDropsTheOldest 钉住丢的是最老的那一帧，不是最新的。
// 实时音频里迟到的帧没有价值，最新的才有。
func TestOverflowDropsTheOldest(t *testing.T) {
	p := parkOutbound(t)

	// 正好灌满，一帧都不该丢。
	for i := 1; i <= outboundDepth; i++ {
		p.enqueue(t, frame(uint16(i), 0))
	}
	if n := p.o.dropped(); n != 0 {
		t.Fatalf("premise: filling the queue to exactly %d already dropped %d frames", outboundDepth, n)
	}

	// 再灌两帧，逼出两次丢弃。两次而不是一次：一次的话"最老"和"次老"
	// 只差一帧，读断言的人分不清丢的是哪一端。
	p.enqueue(t, frame(uint16(outboundDepth+1), 0))
	p.enqueue(t, frame(uint16(outboundDepth+2), 0))
	if n := p.o.dropped(); n != 2 {
		t.Fatalf("dropped() = %d after two overflows, want 2", n)
	}

	// 期望：被按住的那一帧，然后是最新的 outboundDepth 帧——最老的两帧（1 和 2）没了。
	want := []uint16{parkSeq}
	for i := 3; i <= outboundDepth+2; i++ {
		want = append(want, uint16(i))
	}
	got := seqs(t, p.letGo(t, len(want)))
	if !equalSeqs(got, want) {
		t.Fatalf("delivered seqs = %v, want %v — the two oldest frames must be the ones dropped, not the two newest", got, want)
	}
}

// TestTheLastFrameOfABurstIsNeverDropped 钉住 FlagLast 的豁免。
// 灌满队列，队列里含一帧 FlagLast，再继续灌；那一帧必须活到被发出去。
//
// 尾帧刻意放在**队头**：这样一次溢出就足以把两种实现分开——正确的实现跳过它去丢
// 第二老的，"永远丢最老的"那种实现第一次溢出就把它丢了。后面再灌满一整轮，
// 是要证明它扛得住 outboundDepth 次连续溢出，而不是碰巧只活过一次。
func TestTheLastFrameOfABurstIsNeverDropped(t *testing.T) {
	p := parkOutbound(t)

	p.enqueue(t, frame(lastSeq, wire.FlagLast))
	for i := 1; i < outboundDepth; i++ {
		p.enqueue(t, frame(uint16(i), 0))
	}
	for i := outboundDepth; i < 2*outboundDepth; i++ {
		p.enqueue(t, frame(uint16(i), 0))
	}

	// 前提的证明：确实丢了帧。没有这一句，"尾帧还在"在"队列根本没溢出"
	// 的情况下也会通过，而那正是这条断言唯一有意义的场合。
	if n := p.o.dropped(); n != outboundDepth {
		t.Fatalf("dropped() = %d, want %d — without real overflow the FlagLast exemption is never exercised and this test proves nothing", n, outboundDepth)
	}

	want := []uint16{parkSeq, lastSeq}
	for i := outboundDepth + 1; i < 2*outboundDepth; i++ {
		want = append(want, uint16(i))
	}
	sent := p.letGo(t, len(want))

	// 先单独说这一位，因为红的时候要说清楚错在哪：它是接收端熄灭 RX 指示灯用的，
	// 丢了对方的灯就一直亮着，而且没有任何东西会来纠正。
	found := false
	for _, f := range sent {
		h, _, err := wire.Parse(f)
		if err != nil {
			t.Fatalf("delivered frame does not parse: %v", err)
		}
		if h.Seq == lastSeq && h.Flags&wire.FlagLast != 0 {
			found = true
		}
	}
	if !found {
		t.Fatal("the FlagLast frame was dropped; the receiver's RX lamp stays lit with nothing left to extinguish it")
	}

	if got := seqs(t, sent); !equalSeqs(got, want) {
		t.Fatalf("delivered seqs = %v, want %v", got, want)
	}
}

// TestSurvivingFramesKeepTheirOrder 钉住没被丢的帧保持先后顺序。
//
// 刻意**不**让队列溢出（正好灌满），这样这条测试只谈顺序：dropped() == 0 是它的
// 前提，红的时候就一定是出队端把顺序弄反了，而不是丢弃策略挑错了人。
func TestSurvivingFramesKeepTheirOrder(t *testing.T) {
	p := parkOutbound(t)

	for i := 1; i <= outboundDepth; i++ {
		p.enqueue(t, frame(uint16(i), 0))
	}
	if n := p.o.dropped(); n != 0 {
		t.Fatalf("premise: this test must not overflow, but %d frames were dropped", n)
	}

	want := []uint16{parkSeq}
	for i := 1; i <= outboundDepth; i++ {
		want = append(want, uint16(i))
	}
	got := seqs(t, p.letGo(t, len(want)))
	if !equalSeqs(got, want) {
		t.Fatalf("delivered seqs = %v, want %v — the queue must drain front to back", got, want)
	}
}

// TestDropsAreCounted 钉住计数。不断言日志——本仓库没有任何东西解析日志输出，
// 断言日志字段名的测试买不到东西还要维护（已裁定）。
func TestDropsAreCounted(t *testing.T) {
	p := parkOutbound(t)

	for i := 1; i <= outboundDepth; i++ {
		p.enqueue(t, frame(uint16(i), 0))
	}
	if n := p.o.dropped(); n != 0 {
		t.Fatalf("premise: filling the queue to exactly %d already dropped %d frames", outboundDepth, n)
	}

	const extra = 5
	for i := 0; i < extra; i++ {
		p.enqueue(t, frame(uint16(outboundDepth+1+i), 0))
	}
	if n := p.o.dropped(); n != extra {
		t.Fatalf("dropped() = %d after %d overflowing enqueues, want %d", n, extra, extra)
	}
}

// TestStopEndsTheDrainGoroutine 钉住不泄漏 goroutine。
// stop() 返回一个 channel，排空 goroutine 退出时关闭它；测试带上界地等它。
func TestStopEndsTheDrainGoroutine(t *testing.T) {
	delivered := make(chan struct{}, 1)
	o := newOutbound(func([]byte) error {
		select {
		case delivered <- struct{}{}:
		default:
		}
		return nil
	})

	// 前提的证明：排空 goroutine 真的跑起来过。没有这一句，一个根本不起
	// goroutine 的实现也能让下面那条断言通过。
	o.enqueue(frame(1, 0))
	select {
	case <-delivered:
	case <-time.After(waitMax):
		t.Fatal("premise: the drain goroutine never ran at all")
	}

	select {
	case <-o.stop():
	case <-time.After(waitMax):
		t.Fatal("the drain goroutine is still running after stop(); one leaks per connection, and connections churn")
	}

	// 收尾路径可能走两遍（握手失败和正常结束各有一条），第二次必须是空操作
	// 而不是 close of closed channel 的 panic。
	select {
	case <-o.stop():
	default:
		t.Fatal("a second stop() did not return an already-closed channel")
	}
}

// frame 造一个最小的合法数据面包：13 字节包头加一点载荷。
func frame(seq uint16, flags uint8) []byte {
	return append(wire.Header{
		Ver: wire.Version, Flags: flags, Seq: seq, FreqKHz: 121800,
	}.AppendTo(nil), 0xAA, 0xBB)
}

// seqs 把发出去的帧解成 seq 序列，顺带证明它们还是合法的 wire 包——
// 队列不解码载荷，但它也不能把包头弄坏。
func seqs(t *testing.T, frames [][]byte) []uint16 {
	t.Helper()
	out := make([]uint16, 0, len(frames))
	for i, f := range frames {
		h, _, err := wire.Parse(f)
		if err != nil {
			t.Fatalf("delivered frame %d does not parse: %v", i, err)
		}
		out = append(out, h.Seq)
	}
	return out
}

func equalSeqs(a, b []uint16) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// TestDroppingFromAnEmptyQueueDoesNotPanic 钉住 dropOneLocked 的空队列早返回。
//
// 今天没有调用方会这么做——唯一那个先判了 len(o.q) >= outboundDepth——所以这条
// 测试是**唯一**能让那行早返回被删掉时变红的东西。而它值得钉：没有早返回时
// o.q[victim+1:] 是 o.q[1:0]，实测 `slice bounds out of range [1:0]`，而且它
// panic 在扇出线程上（enqueue 是扇出调的），整条会话跟着走。
//
// 刻意直接调私有方法并自己持锁：绕过那个前置条件正是这里要模拟的事——
// 将来某个"背压时主动排空"的助手不会知道有这么个不成文的规矩。
func TestDroppingFromAnEmptyQueueDoesNotPanic(t *testing.T) {
	o := newOutbound(func([]byte) error { return nil })
	defer o.stop()

	o.mu.Lock()
	if len(o.q) != 0 {
		o.mu.Unlock()
		t.Fatal("premise: a freshly built queue must be empty")
	}
	o.dropOneLocked()
	o.mu.Unlock()

	// 没丢掉任何东西，所以也不该记一笔。
	if n := o.dropped(); n != 0 {
		t.Fatalf("dropped() = %d after dropping from an empty queue, want 0 — nothing was there to drop", n)
	}
}
