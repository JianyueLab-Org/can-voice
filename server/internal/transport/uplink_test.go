package transport

import (
	"testing"
	"time"
)

// 本文件钉住上行限速的算术。时间全部是**注入的**：限速要拦的那个失控循环每秒
// 几千帧，而要放行的那条 ATIS 上行一播就是几天——两件事都没法靠真的等来测。
//
// 接线（readDatagrams 真的问过这个桶、超了真的发 NOTICE）在 conn_test.go，
// 那一条走真实的 QUIC 连接。这里只管"给定时间和帧数，放不放"。

// base 是所有用例的起点。取一个固定时刻而不是 time.Now()，
// 让失败信息里的时间是可读的。
var base = time.Date(2026, 9, 21, 0, 0, 0, 0, time.UTC)

// opusFrame 是一帧的时长。50 帧每秒的另一种写法，写成时长是因为下面的循环按
// 时间推进而不是按帧数。
const opusFrame = time.Second / uplinkFramesPerSecond

// TestASteadyPttAtTheOpusFrameRateIsNeverLimited 是这个桶的第一要求：
// **正常说话永远不能被它碰到。**
//
// 一分钟的 PTT 是 3000 帧。任何一帧被丢掉，听众那边都是一次可听见的破音，
// 而故障看起来像网络问题——限速器是最后一个会被怀疑的东西。
func TestASteadyPttAtTheOpusFrameRateIsNeverLimited(t *testing.T) {
	l := newUplinkLimiter(1, base)
	now := base
	for i := range 3000 {
		now = now.Add(opusFrame)
		if !l.allow(now) {
			t.Fatalf("frame %d of a steady %d fps uplink was dropped after %v; this is exactly the traffic a real PTT makes",
				i, uplinkFramesPerSecond, now.Sub(base))
		}
	}
}

// TestAContinuousUplinkWithASlightlyFastClockIsNotLimited 钉住余量存在的理由。
//
// 上行的节拍来自客户端的声卡时钟，限速的节拍来自服务端的 time.Now()。两边差
// 百分之几是常态，而 ATIS 席位的上行**永不停止**——一播几天。余量取 1.0 的话
// 桶会以那个差值慢慢见底，于是一条完全正常的通播在几十秒之后开始丢帧，而且是
// 渐进的、听起来正是网络不好的样子。
//
// 一小时的虚拟时间：真实故障就发生在这个量级上，跑几秒的测试看不见它。
func TestAContinuousUplinkWithASlightlyFastClockIsNotLimited(t *testing.T) {
	// 声卡快 5%，帧间隔相应缩短。整数算而不是乘一个浮点，免得这条测试自己
	// 落在"余量恰好够"的边界上。
	const driftPercent = 5
	interval := opusFrame * 100 / (100 + driftPercent)

	l := newUplinkLimiter(1, base)
	now := base
	for i := range int(time.Hour / interval) {
		now = now.Add(interval)
		if !l.allow(now) {
			t.Fatalf("a continuous uplink whose clock runs %d%% fast was dropped at frame %d (%v in); an ATIS station transmits for days, so any deficit per frame eventually empties the bucket — uplinkRateHeadroom is what stops that",
				driftPercent, i, now.Sub(base))
		}
	}
}

// TestTheRateScalesWithTheDeclaredTxWidth 钉住"每秒 50 帧"是**每个频率**的。
//
// 一次 PTT 在客户端那边是每个 TX 频率各发一份上行（pump.rs 那个 `for freq in
// subs.acknowledged().tx`，router/fanout.go 的耦合注释也说了同一件事）。所以一个
// 在 8 个频率上发射的管制员每秒发 400 帧，全都是正常音频。按一个频率的额度去卡，
// 会把耦合席位——也就是真管制员——挑出来限速。
func TestTheRateScalesWithTheDeclaredTxWidth(t *testing.T) {
	const width = 8
	l := newUplinkLimiter(width, base)
	now := base
	for tick := range 1500 { // 30 秒
		now = now.Add(opusFrame)
		for freq := range width {
			if !l.allow(now) {
				t.Fatalf("tick %d, frequency %d of %d was dropped after %v; a controller transmitting on %d coupled frequencies sends one datagram per frequency per frame and all of it is real audio",
					tick, freq, width, now.Sub(base), width)
			}
		}
	}
}

// TestTheSameTrafficFromASingleFrequencySessionIsLimited 是上一条的反面。
//
// 额度跟着授权后的 MaxTX 走，不是跟着对端发了多少走：一条只声明了一个频率的
// 会话发出八倍的量，那八倍**不可能**是音频。两条一起看才说明这个数是有意义的
// ——只有上一条的话，把额度设成无穷大也照样绿。
func TestTheSameTrafficFromASingleFrequencySessionIsLimited(t *testing.T) {
	l := newUplinkLimiter(1, base)
	now := base
	for range 1500 {
		now = now.Add(opusFrame)
		for range 8 {
			if !l.allow(now) {
				return
			}
		}
	}
	t.Fatalf("a session that declared one transmit frequency sent %d fps for 30 s and nothing was dropped; the limit is not doing anything",
		8*uplinkFramesPerSecond)
}

// TestARunawayUplinkIsDroppedAfterTheBurst 是这个 issue 本身：一个卡住的 PTT
// 发包循环失控，能把同频率上每个听众队列里的正常语音挤掉
// （outbound.go 的队列满了丢最旧的）。
//
// 时间一步不推进，所以放行的份数就是桶的容量，不多不少。
func TestARunawayUplinkIsDroppedAfterTheBurst(t *testing.T) {
	const width = 4
	l := newUplinkLimiter(width, base)
	allowed := 0
	for range 100000 {
		if l.allow(base) {
			allowed++
		}
	}
	if want := uplinkBurstFrames * width; allowed != want {
		t.Fatalf("a runaway loop got %d frames through in zero time, want exactly the burst %d (%d frames × %d transmit frequencies)",
			allowed, want, uplinkBurstFrames, width)
	}
}

// TestALimitedSessionRecoversOnceItSlowsDown 钉住"被限住"不是一个吸收态。
//
// 触发限速的多半是客户端的一个 bug，而它会被修好、会重来、也可能只是抖了一下。
// 服务端这一侧不断开连接（断开是不对称的：对端只看得见掉线，于是重连、重放，
// 形成死循环，见 control.KindUnknownMessage），所以桶必须自己回血——否则这条
// 会话要到下一次重连才能再说话，而没有任何东西会告诉它去重连。
func TestALimitedSessionRecoversOnceItSlowsDown(t *testing.T) {
	l := newUplinkLimiter(1, base)
	// 抽干，但循环要有上界：没上界的话，一个"永远放行"的实现会让这条测试
	// **挂住**而不是变红，而挂住的测试在 CI 里只是一次超时，没人看得出是谁。
	drained := false
	for range 10 * uplinkBurstFrames {
		if !l.allow(base) {
			drained = true
			break
		}
	}
	if !drained {
		t.Fatalf("premise: %d frames in zero time and the bucket still had tokens, so there is nothing to recover from", 10*uplinkBurstFrames)
	}

	// 一帧的时间只够长出一帧多一点的额度（余量 1.5 倍），足以放行一帧。
	now := base.Add(opusFrame)
	if !l.allow(now) {
		t.Fatalf("a session that was limited and then went quiet for one frame (%v) is still refused; nothing else would ever let it speak again", opusFrame)
	}

	// 而且恢复之后是**完整**恢复：接着按正常速率说一分钟，一帧都不该再丢。
	for i := range 3000 {
		now = now.Add(opusFrame)
		if !l.allow(now) {
			t.Fatalf("frame %d of a normal uplink was dropped %v after the session recovered; the bucket is still carrying the debt from the burst", i, now.Sub(base))
		}
	}
}

// TestASessionThatMayNotTransmitStillGetsTokens 钉住额度不会是零。
//
// MaxTX 为 0 的会话（纯听众）一帧都不该发，而它发来的包本该拿到 tx_denied——
// 那条 NOTICE 是它唯一能看见的诊断。额度按 MaxTX 直接乘的话这里就是 0，于是
// 限速排在 Fanout 前面把包吃掉，tx_denied 永远发不出去：一个配错了的客户端
// 从此得到的是错误的答案。
func TestASessionThatMayNotTransmitStillGetsTokens(t *testing.T) {
	l := newUplinkLimiter(0, base)
	if !l.allow(base) {
		t.Fatal("a session with MaxTX 0 got no tokens at all, so its packets are dropped before the router can answer tx_denied")
	}
}
