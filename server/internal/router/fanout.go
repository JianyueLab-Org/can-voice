// 本文件是 router 的扇出路径：一个上行数据包如何变成若干个下行数据包。
//
// 三条贯穿全文的规则：
//   - 载荷全程不解码。服务端没有 Opus 依赖，音频对它是不透明字节。
//   - 所有"不知道"的分支一律放行。在语音系统里"听不见"比"听得太远"糟糕得多。
//   - 一个听众在一次扇出里最多收到一份。两份就是回声。
package router

import (
	"fmt"
	"slices"

	"github.com/JianyueLab-Org/can-voice/server/internal/fsdfeed"
	"github.com/JianyueLab-Org/can-voice/server/internal/geo"
	"github.com/JianyueLab-Org/can-voice/server/internal/wire"
)

// Locator 提供位置快照。抽成接口是为了让扇出的测试不必碰网络。
type Locator interface {
	Snapshot() fsdfeed.Snapshot
	Degraded() bool
}

// SetLocator 装上位置来源。不装等于永久降级：全部扇出。
func (r *Router) SetLocator(l Locator) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.locator = l
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
		return 0, fmt.Errorf("session %d has not declared transmit on %d", from, h.FreqKHz)
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
	targets := append([]uint32{h.FreqKHz}, r.coupledWith(h.FreqKHz)...)

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
			// 的管制员因此只收到一份——两份就是回声。
			seen[l.ID] = struct{}{}

			qual, deliver := qualityFor(snap, degraded, senderPos, senderKnown, l)
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
		}
	}
	return n, nil
}

// qualityFor 判定一个听众该不该收到这一包，以及收到时的信号质量。
//
// 所有"不知道"的分支都汇到这里，答案统一是**放行**而不是屏蔽：
// 在语音系统里"听不见"比"听得太远"糟糕得多，而"位置未知"恰好就是一架
// 刚连上、还没发位置包、正要呼叫放行的飞机所处的状态。
func qualityFor(snap fsdfeed.Snapshot, degraded bool, senderPos fsdfeed.Position, senderKnown bool, l *Session) (uint8, bool) {
	if degraded || !senderKnown {
		return 255, true
	}
	lp, ok := lookup(snap, l)
	if !ok {
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

func (r *Router) positions() (fsdfeed.Snapshot, bool) {
	r.mu.RLock()
	l := r.locator
	r.mu.RUnlock()
	if l == nil {
		// 没装 Locator 等于永久降级：全部放行。启动顺序里 Fanout 完全
		// 可能先于 SetLocator。
		return fsdfeed.Snapshot{}, true
	}
	return l.Snapshot(), l.Degraded()
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
