// 本文件是 router 的扇出路径：一个上行数据包如何变成若干个下行数据包。
//
// 三条贯穿全文的规则：
//   - 载荷全程不解码。服务端没有 Opus 依赖，音频对它是不透明字节。
//   - 所有"不知道"的分支一律放行。在语音系统里"听不见"比"听得太远"糟糕得多。
//   - 一个听众在**一个频率上**最多收到一份。同一个频率上的两份就是回声。
//     跨频率不是：同时订阅一对耦合频率两边的人会在两边各收到一份，那是无线电栈
//     上的两行，见下面 targets 那一段和 wire.Header.Seq 的契约。
package router

import (
	"fmt"
	"slices"

	"github.com/JianyueLab-Org/can-voice/server/internal/fsdfeed"
	"github.com/JianyueLab-Org/can-voice/server/internal/geo"
	"github.com/JianyueLab-Org/can-voice/server/internal/wire"
)

// Locator 提供位置快照。抽成接口是为了让扇出的测试不必碰网络。
//
// **一次调用同时给出两个值，不是两个方法。** 分成 Snapshot() 和 Degraded()
// 的话，扇出要分两次拿锁，中间可以插进一次整体替换：拿到的会是"新的快照配
// 旧的降级位"或者反过来。最难看的那一半是后者——feed 刚刚断线，Degraded()
// 已经是 true 了，而上一次调用拿到的还是那份完整快照，于是这一轮扇出既不
// 全放行、又按一份马上就要作废的位置做了射程过滤。
//
// 这正是 Fanout 自己"快照只取一次"那段注释里的同一条理由（见下面），只是那
// 一段说的是跨频率、这一段说的是跨字段。同一个不变量落实在两个地方，不能只
// 落实一处。
type Locator interface {
	Positions() (fsdfeed.Snapshot, bool)
}

// SetLocator 装上位置来源。不装等于永久降级：全部扇出。
func (r *Router) SetLocator(l Locator) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.locator = l
	r.locatorEpoch++
}

// TxDeniedError 是"这个会话没有在这个频率上声明发射"。
//
// 是一个类型而不是一句格式化好的话：传输层要把频率原样放进 NOTICE，
// 而从一句话里再把它解析出来，是先把它丢掉再捡回来。
type TxDeniedError struct {
	Session SessionID
	FreqKHz uint32
}

func (e *TxDeniedError) Error() string {
	return fmt.Sprintf("session %d has not declared transmit on %d", e.Session, e.FreqKHz)
}

// Fanout 把一个上行数据包转发给订阅者，返回实际投递的份数。
//
// 路径（spec 8）：校验发送权 → 算出目标频率 → 查订阅者 → 逐个算 qual →
// 填 speaker/qual/freq 后原样转发。**载荷全程不解码**——服务端没有 Opus
// 依赖，音频对它是不透明字节。
func (r *Router) Fanout(from SessionID, packet []byte) (int, error) {
	h, opus, err := wire.Parse(packet)
	if err != nil {
		return 0, err
	}
	// 第一道校验：没声明在这个频率上发送就丢弃，
	// 否则任何人都能往任意频率喊话。
	if !r.MayTransmit(from, h.FreqKHz) {
		return 0, &TxDeniedError{Session: from, FreqKHz: h.FreqKHz}
	}
	sender, ok := r.Get(from)
	if !ok {
		return 0, fmt.Errorf("session %d is gone", from)
	}

	// 快照只取一次。每个频率各取一次的话，一次交叉耦合扇出的不同频率可能
	// 用到不同的快照，同一个人在两个频率上得到不同的射程判定。
	snap, degraded := r.positions()
	var senderPos fsdfeed.Position
	var senderKnown bool
	if !degraded {
		senderPos, senderKnown = lookup(snap, sender)
	}

	// 目标频率：主频率在前，耦合频率升序在后。顺序是确定的，因为它决定了
	// 同时订阅多个目标频率的听众收到的包头上写哪个频率。
	//
	// **耦合频率里要扣掉发言者自己也在发射的那几个。** normaliseXC 要求一对
	// 耦合的两个频率都在该会话**授权后**的 TX 集合里，所以"声明了 XC(A,B)"蕴含
	// "在 A 和 B 上都有发射权"——那不是一种可能的配置，是唯一能让耦合成立的配置。
	// 而无线电栈形状的客户端（TrackAudio、can-audio）一次 PTT 会给每个 TX 频率
	// 各发一份上行：can-audio 的 voice.py 里 PTT 独占 VoiceTarget id 1，那个
	// target 装着全部 TX 频道。两份上行各走一次 Fanout，而下面那个 seen 去重
	// 只在**单次调用之内**有效，两次调用互相不知道对方存在。于是只订阅 B 的
	// 听众会收到两份同号的包——一份是 B 上的直投，一份是从 A 耦合过来的——
	// 而契约里"按 (Speaker, FreqKHz) 分流之后是一串连号"就变成了假话：
	// 实际收到的是 8, 8, 9, 9, 10, 10。
	//
	// 扣掉那一份是纯本地判断，不需要任何跨调用的状态：发言者自己就在那个频率上
	// 直发，耦合拷贝按构造是多余的。协议一个字节都不用改。
	//
	// 代价是客户端那一半从此是承重的：**声明了 XC 就必须真的每个 TX 频率各发
	// 一份**，服务端不会替它把音频搬到它自己也在发射的那个频率上去。这条写在
	// wire.Header.Seq 的契约里，Rust 侧照那段实现。
	senderTX := sender.subs.Load().tx
	targets := []uint32{h.FreqKHz}
	for _, f := range r.coupledWith(h.FreqKHz) {
		if _, own := senderTX[f]; own {
			continue
		}
		targets = append(targets, f)
	}

	// 发言者一开始就算"已投递"，这样他既不会收到自己的声音（TX 蕴含 RX，
	// 他是自己的订阅者），也不会在耦合频率上被再考虑一次。
	seen := map[SessionID]struct{}{sender.ID: {}}
	n := 0
	for _, freq := range targets {
		// 每个频率都重新调 Listeners：它返回的是一份新切片，跨频率缓存下来
		// 的话，中途被顶掉的会话会留在那份切片里继续收包。
		for _, l := range r.Listeners(freq) {
			if _, dup := seen[l.ID]; dup {
				continue
			}
			// 在判定之前就登记：射程只取决于双方的位置，和这个包走哪个
			// 频率无关，所以一个听众只判一次。同时订阅了主频率和耦合频率
			// 的管制员因此**在这一个上行包上**只收到一份——同一个频率上的
			// 两份就是回声。跨上行包的那一半由上面扣掉自己 TX 频率的那段
			// 管，seen 到不了那里。
			seen[l.ID] = struct{}{}

			lp, listenerKnown := lookup(snap, l)
			qual, deliver := qualityFor(degraded, senderPos, senderKnown, lp, listenerKnown)
			if !deliver {
				continue
			}
			out := wire.Header{
				Ver:     wire.Version,
				Flags:   h.Flags,
				Qual:    qual,
				Seq:     h.Seq,
				FreqKHz: freq,
				Speaker: uint32(sender.ID),
			}.AppendTo(nil)
			// 每个听众一份新缓冲：Send 可能是异步的（传输层把它塞进
			// QUIC 的发送队列），共用一份缓冲就是 use-after-free 那一类。
			l.Send(append(out, opus...))
			n++
			if r.noteTalker(l.ID, sender.ID) && l.notifyTalker != nil {
				l.notifyTalker(sender.ID, sender.CID, freq)
			}
		}
	}
	return n, nil
}

// qualityFor 判定一个听众该不该收到这一包，以及收到时的信号质量。
//
// 所有"不知道"的分支都汇到这里，答案统一是**放行**而不是屏蔽：
// 在语音系统里"听不见"比"听得太远"糟糕得多，而"位置未知"恰好就是一架
// 刚连上、还没发位置包、正要呼叫放行的飞机所处的状态。
//
// 两个参与者都以 (位置, 有没有查到) 的形式传进来，对称——而且刻意**不**在这里
// 查快照。查表留在 Fanout 里，是为了让这个函数成为两组位置的纯函数，否则它自己
// 那两道"不知道就放行"的闸门根本钉不住：查表在里面的话，!ok 那一支拿到的永远是
// 零值 Position（Known 为 false），于是 fsdfeed.EffectiveRangeNM 会替它放行，
// 任何测试都分不出闸门在不在。代价是降级时也会为每个听众查一次表（之前可以
// 短路掉），一次 map 查询几十纳秒，换一道能被钉住的闸门，值。
func qualityFor(degraded bool, senderPos fsdfeed.Position, senderKnown bool, lp fsdfeed.Position, listenerKnown bool) (uint8, bool) {
	if degraded || !senderKnown {
		return 255, true
	}
	if !listenerKnown {
		// 只连了语音没连 FSD 的人不该因此变成聋子。
		return 255, true
	}
	// 合并规则只存在于 EffectiveRangeNM 一处：飞行员存的是视距**半项**，
	// 席位存的是权威半径，两种量不能互换，这里绝不能自己写 min/max/相加。
	// filter 为 false 表示"我们不知道"，放行而不是丢弃。
	rng, filter := fsdfeed.EffectiveRangeNM(senderPos, lp)
	if !filter {
		return 255, true
	}
	d := geo.DistanceNM(senderPos.Lat, senderPos.Lon, lp.Lat, lp.Lon)
	return geo.Quality(d, rng)
}

// coupledWith 返回与 freq 交叉耦合的频率，升序。
//
// 耦合是全服务端的，不是发送方一个人的：任何人在 A 上发言都会到达 B 的
// 订阅者。这是交叉耦合在管制上的本义——把两个频率当一个用（清闲时段一个人
// 同时管 DEL 和 GND）。只转发耦合方自己的发言的话，A 上的飞行员说的话到不了
// B，两个频率并没有合成一个会话，而 can-audio 今天实现的正是合成。
//
// 只走一跳，不做传递闭包：A↔B 且 B↔C 时 A 不到 C。否则一个成环的声明会在
// 每秒跑几万次的路径上打转。
func (r *Router) coupledWith(freq uint32) []uint32 {
	r.mu.RLock()
	defer r.mu.RUnlock()
	m := r.xc[freq]
	if len(m) == 0 {
		return nil
	}
	out := make([]uint32, 0, len(m))
	for f := range m {
		out = append(out, f)
	}
	// 排序是为了确定性：目标频率的顺序决定了同时订阅多个目标频率的听众
	// 收到的包头上写的是哪个频率，而 Go 的 map 遍历是随机的。
	slices.Sort(out)
	return out
}

// PositionsDegraded 报告射程过滤此刻是否降级。降级时全部放行。
//
// 传输层拿它告诉发言者一声：降级是正确的取舍，但它不该是一件悄悄发生的事。
func (r *Router) PositionsDegraded() bool {
	_, degraded := r.positions()
	return degraded
}

func (r *Router) positions() (fsdfeed.Snapshot, bool) {
	r.mu.RLock()
	l := r.locator
	r.mu.RUnlock()
	return positionsFromLocator(l)
}

func (r *Router) positionsWithEpoch() (fsdfeed.Snapshot, bool, uint64) {
	r.mu.RLock()
	l, epoch := r.locator, r.locatorEpoch
	r.mu.RUnlock()
	snap, degraded := positionsFromLocator(l)
	return snap, degraded, epoch
}

func positionsFromLocator(l Locator) (fsdfeed.Snapshot, bool) {
	if l == nil {
		// 没装 Locator 等于永久降级：全部放行。启动顺序里 Fanout 完全
		// 可能先于 SetLocator。
		return fsdfeed.Snapshot{}, true
	}
	return l.Positions()
}

// lookup 找一个会话的位置。观察员模式下 Follow 指向它跟随的飞机，
// 因为观察员自己没有 FSD 连接（spec 7.3）。
func lookup(s fsdfeed.Snapshot, sess *Session) (fsdfeed.Position, bool) {
	if sess.Follow != "" {
		p, ok := s.ByCallsign[sess.Follow]
		return p, ok
	}
	p, ok := s.ByCID[sess.CID]
	return p, ok
}
