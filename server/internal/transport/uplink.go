package transport

import "time"

// 本文件是上行音频的每会话限速。
//
// 为什么要有它：出站队列满了丢**最旧**的一帧（outbound.go 的 enqueue）。那个取舍
// 对拥塞是对的，但它意味着一路失控的上行——一个卡住的 PTT 让发包循环转起来就够
// 了——会把同一个频率上每个听众队列里正在播的语音挤掉。受害的不是那个出故障的
// 人，是频率上的所有人，而他们那边的症状是"今天电台不太干净"。
//
// 旧版不需要写这个：Murmur 自带 `bandwidth` / `messagelimit` / `messageburst`
// （can-audio 的 `server/whereami.py` 把后两项列为排障时要看的配置）。自己写服务端
// 就得自己带上。
//
// 发言要先拿票，所以这不是一个匿名入口，限速要拦的也不是攻击者，而是一个跑飞了的
// 客户端。这决定了两件事：**丢包不断连**（断开是不对称的，对端只看得见掉线，于是
// 重连、重放，形成死循环——见 control.KindUnknownMessage 那段），以及**桶必须自己
// 回血**，否则一次抖动会把一条会话按到下一次重连为止。

// uplinkFramesPerSecond 是一条上行在**一个频率上**的正常速率。
//
// 50 = 1 秒 / 20 ms。Opus 帧长在这个网络里是定死的，不是一个惯例：客户端的
// `crates/can-voice-client/src/tx/mod.rs` 只收 960 采样（48 kHz 的 20 ms）的帧，
// 别的长度直接报错。outbound.go 的队列深度也是按这个数算的。
const uplinkFramesPerSecond = 50

// uplinkRateHeadroom 是限速相对上面那个正常速率的倍数。
//
// **不能取 1.0。** 上行的节拍来自客户端的声卡时钟，限速的节拍来自服务端的
// time.Now()，两边差千分之几到百分之几是常态；而 ATIS 席位的上行永不停止，一播
// 就是几天。余量取 1.0 的话，桶会以那个差值慢慢见底，于是一条完全正常的通播在
// 几十秒到几分钟之后开始丢帧——渐进的、断断续续的、听起来和网络不好一模一样，
// 而限速器是最后一个会被怀疑的东西。
//
// 1.5 的另一头是它仍然拦得住要拦的那个东西：一个失控的发包循环快的是几十倍，
// 不是一点五倍。这中间没有需要精细调校的地带。
const uplinkRateHeadroom = 1.5

// uplinkBurstFrames 是突发额度，同样按**每个可发射频率**计。
//
// 25 帧 = 半秒。要的是吸收成簇到达：网络和调度会把本该每 20 ms 一帧的上行攒成
// 一小簇（Wi-Fi 的聚合、客户端线程被抢占之后一次补发），按帧卡的话这些全是误伤。
// 取半秒而不是更多，是因为突发额度就是失控客户端一次能放出来的量，而听众那边的
// 队列只有 outboundDepth 帧深。
const uplinkBurstFrames = 25

// uplinkLimiter 是一条会话的令牌桶。
//
// **只有 readDatagrams 那一条 goroutine 碰它**，和那里的 denied / toldDegraded
// 一样，所以没有锁。时间由调用方传进来：正常路径上 time.Now() 已经要取一次，
// 而测试要能把一小时的连续上行在一瞬间跑完。
type uplinkLimiter struct {
	// rate 是每秒补多少令牌，burst 是桶的容量。两者都在构造时按授权后的
	// MaxTX 算好，此后不变。
	rate   float64
	burst  float64
	tokens float64
	last   time.Time
}

// newUplinkLimiter 按这条会话**授权后**的 MaxTX 算出额度。
//
// 乘 MaxTX 是承重的，不是宽松一点而已：一次 PTT 在客户端那边是每个 TX 频率各发
// 一份上行（pump.rs 的 `for freq in subs.acknowledged().tx`，router/fanout.go 的
// 耦合注释说的是同一件事）。一个在 8 个频率上发射的管制员每秒 400 帧全都是真音频，
// 按单频率的额度去卡，会把耦合席位——也就是真管制员——挑出来限速。
//
// 用的是 grantedMaxTX 之后的值，所以它已经被服务端夹过（≤ serverMaxTX，≤ MaxRX），
// 一张 max_tx 写得离谱的 token 换不到更大的桶。
//
// 下界 1：MaxTX 为 0 的纯听众一帧都不该发，而它发来的包本该拿到 tx_denied——那条
// NOTICE 是它唯一看得见的诊断。额度为 0 的话，限速排在 Fanout 前面把包吃掉，
// 它拿到的就是"限速"这个错误答案。
func newUplinkLimiter(maxTX int, now time.Time) *uplinkLimiter {
	width := float64(max(maxTX, 1))
	burst := uplinkBurstFrames * width
	return &uplinkLimiter{
		rate:   uplinkFramesPerSecond * uplinkRateHeadroom * width,
		burst:  burst,
		tokens: burst,
		last:   now,
	}
}

// allow 取一个令牌，取不到返回 false。
//
// 不做"把时间倒流的情况也算对"这种事：now 来自 time.Now()，Go 的时刻带单调钟，
// 同一个进程里不会倒退。真要倒退了，下面那个 d > 0 也只是不补令牌而已。
func (l *uplinkLimiter) allow(now time.Time) bool {
	if d := now.Sub(l.last); d > 0 {
		l.last = now
		l.tokens = min(l.tokens+d.Seconds()*l.rate, l.burst)
	}
	if l.tokens < 1 {
		return false
	}
	l.tokens--
	return true
}
