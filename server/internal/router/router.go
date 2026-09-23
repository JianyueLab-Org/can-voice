// Package router 是 can-voice 的全部业务状态：会话、订阅表、交叉耦合。
//
// 它不认识 QUIC——发送通过注入的回调完成，所以整个路由逻辑可以纯逻辑测试。
// 没有数据库、没有持久化：进程重启等于所有人重连并重发 SUB。
//
// 并发契约，两层：
//   - Router 自己的两张表（sessions、rx 倒排索引）由 mu 保护。
//   - 每条会话的订阅状态由该会话的原子指针保护，可以在 Router 的锁外读。
//     这是必须的：Listeners() 把 *Session 交出去之后，扇出就在锁外了。
package router

import (
	"log/slog"
	"slices"
	"sync"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/control"
)

// Router 持有全部服务端状态。
type Router struct {
	mu       sync.RWMutex
	sessions map[SessionID]*Session
	// rx 是频率到订阅者的倒排索引，扇出时只查这一张表。
	rx map[uint32]map[SessionID]struct{}
	// byCID 是**(成员号, 席位)** 到当前会话的索引，只为顶号服务。
	// 键由 evictionKey 拼，见那里为什么不是光一个成员号。
	// 一个 CID 最多一条会话，所以是 SessionID 而不是集合。
	byCID map[string]SessionID
	// locator 是位置来源，扇出时查它。为 nil 等于永久降级：全部放行。
	locator Locator
	// announced：听众 → 已经向他介绍过的发言者。每个听众对每个发言者只发一次
	// talker 通知（#46）。键是会话号，断线就清。
	announced map[SessionID]map[SessionID]struct{}
	// xc 是频率到与之交叉耦合的频率的派生索引，值是引用计数。
	//
	// 计数而不是布尔：几个管制员可能各自声明同一对，其中一个撤销时这一对
	// 必须还在。和 rx 倒排索引一样由 Subscribe 整体重建。
	xc map[uint32]map[uint32]int
}

// New 建一个空的 Router。
func New() *Router {
	return &Router{
		sessions:  map[SessionID]*Session{},
		rx:        map[uint32]map[SessionID]struct{}{},
		byCID:     map[string]SessionID{},
		xc:        map[uint32]map[uint32]int{},
		announced: map[SessionID]map[SessionID]struct{}{},
	}
}

// SessionOpts 是建立一条会话需要的全部东西。
//
// 用结构体而不是位置参数，是因为这里有两对同类型的相邻参数
// （cid/follow 都是 string，maxTX/maxRX 都是 int）——写反了编译器一声不响，
// 而 maxTX 和 maxRX 写反的表现是"某些人莫名其妙不能讲话"。
type SessionOpts struct {
	// CID 是成员号，来自已验签的 token。
	CID string
	// Follow is for legacy router-only tests. Production rejects it at HELLO.
	Follow string
	// Station 是同一个账号下的第几个席位，空串表示"就一个"。
	//
	// **通播机队整队共用一个 ATIS 账号**，而顶号按 CID。没有这个标记的话，
	// 两个以上 `_ATIS` 席位时同一时刻只有一路在播：被顶掉的那一路重连、
	// 再顶掉下一个，全队每秒互踢一轮，而两端日志都写着"成功"。
	// 设计文档 §6 说的"普通会话，带一个 station 标记"就是这个。
	Station string
	// Signed voice scope. Empty role is reserved for router-only legacy tests.
	Role         string
	Callsign     string
	TXGrant      []uint32
	GrantExpires time.Time
	// MaxTX 来自 token：最多能在几个频率上发送。
	MaxTX int
	// MaxRX 来自服务端配置：最多能订阅几个频率。
	MaxRX int
	// Send 把一个数据面包发给这个会话。
	Send func([]byte)
	// NotifyTalker 告诉这个听众：会话 speaker 的 CAN 号是 cid。
	// 每个发言者只调一次。可以为 nil（纯逻辑测试里就是）。
	NotifyTalker func(speaker SessionID, cid string, freq uint32)
	// Close 断开底层连接。可以为 nil（纯逻辑测试里就是）。
	Close func()
}

// evictionKey 是顶号的键。
//
// **不是光一个成员号。** 通播机队整队共用一个 ATIS 账号，每一路席位一个
// station 标记；按 CID 顶的话它们会互相踢，而按 (CID, station) 顶的时候，
// 同一个席位重启仍然顶掉自己上一条——那正是顶号本来要解决的问题。
//
// 分隔符用 NUL：它不可能出现在成员号或呼号里，所以拼不出两个不同的
// (cid, station) 撞成同一个键。
func evictionKey(cid, station string) string {
	return cid + "\x00" + station
}

// Add 登记一条新会话，并顶掉同一个 CID 的旧会话（如果有）。
//
// 顶号不是可选的。不顶的话，半开连接掉线重连的成员会有最长一个 MaxIdleTimeout
// 两条会话同时订阅同一批频率，而"不回声给自己"判的是 SessionID 不是 CID——
// 于是他听到自己的声音，延迟约一个 RTT，持续到旧连接超时为止。can-audio 的
// server/login.py 一直是踢的，观察员模式要求两名机组各用各的账号正是基于这一点。
//
// 新会话带着一个空声明入场，不是 nil——扇出完全可能在第一个 SUB 到达之前
// 就看到它，那时 subs.Load() 必须已经可用。
func (r *Router) Add(o SessionOpts) *Session {
	grant := make(map[uint32]struct{}, len(o.TXGrant))
	for _, f := range o.TXGrant {
		grant[f] = struct{}{}
	}
	s := &Session{
		ID:           newSessionID(),
		CID:          o.CID,
		Follow:       o.Follow,
		Station:      o.Station,
		Role:         o.Role,
		Callsign:     o.Callsign,
		TXGrant:      grant,
		GrantExpires: o.GrantExpires,
		MaxTX:        o.MaxTX,
		MaxRX:        o.MaxRX,
		send:         o.Send,
		notifyTalker: o.NotifyTalker,
		closeConn:    o.Close,
	}
	s.subs.Store(emptySubs())

	r.mu.Lock()
	var evicted *Session
	if o.CID != "" {
		// 空 CID 不参与顶号。它不该出现（鉴权拒绝空 CID），但真出现时让所有
		// 空 CID 会话互相顶掉，比放过去糟糕得多。
		if prev, ok := r.byCID[evictionKey(o.CID, o.Station)]; ok {
			evicted = r.sessions[prev]
			r.removeLocked(prev)
		}
		r.byCID[evictionKey(o.CID, o.Station)] = s.ID
	}
	r.sessions[s.ID] = s
	if o.Role != "" && !o.GrantExpires.IsZero() {
		s.expiryTimer = time.AfterFunc(time.Until(o.GrantExpires), r.ReconcileAuthority)
	}
	r.mu.Unlock()
	// 断连放在锁外：closeConn 是传输层的回调，它可能回头再调 router
	// （比如它自己的 defer 里有 Remove），持锁调用就是自锁。
	if evicted != nil {
		// 日志在 nil 判断**之外**：顶号这件事发生了就该留痕，
		// 有没有连接可断是另一回事（纯逻辑测试里就没有）。
		slog.Info("evicting an earlier session for the same cid",
			"cid", o.CID, "evicted", evicted.ID, "new", s.ID)
		if evicted.closeConn != nil {
			evicted.closeConn()
		}
	}
	return s
}

// Remove 注销会话并把它从每一个索引里摘掉。
//
// 不调用 closeConn：调用方就是传输层，它正在拆这条连接，回调进去等于让它
// 自己拆自己。只有顶号那条路径才需要主动断开（那时对方并不知道自己该走了）。
func (r *Router) Remove(id SessionID) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.removeLocked(id)
}

// removeLocked 是 Remove 的持锁本体，供 Add 的顶号路径复用。
func (r *Router) removeLocked(id SessionID) {
	s, ok := r.sessions[id]
	if !ok {
		return
	}
	if s.expiryTimer != nil {
		s.expiryTimer.Stop()
		s.expiryTimer = nil
	}
	old := s.subs.Load()
	for f := range old.rx {
		r.unindex(f, id)
	}
	// 耦合索引也要摘，理由和 rx 一样：它是这条声明的派生物。漏掉的话，一个
	// 管制员断线之后他声明的那一对永远留在索引里，两个频率从此被永久接通，
	// 而声明它的人已经不在了。
	r.bumpXC(old.xc, -1)
	s.subs.Store(emptySubs())
	// cid 索引也要清，否则留下一个指向已注销会话的条目，下一次同 cid 登录
	// 会对着它调 Close，而那个闭包捕获的是一条已经关掉的连接。
	// 只在它确实指向这条会话时才删——顶号路径上新条目稍后才写入，
	// 但别的顺序下这个判断是防止误删的那道闸。
	if cur, ok := r.byCID[evictionKey(s.CID, s.Station)]; ok && cur == id {
		delete(r.byCID, evictionKey(s.CID, s.Station))
	}
	delete(r.sessions, id)
	delete(r.announced, id)
	for _, m := range r.announced {
		delete(m, id)
	}
}

// noteTalker 记下听众 L 已经知道发言者 S。第一次返回 true。
func (r *Router) noteTalker(listener, speaker SessionID) bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	if _, ok := r.sessions[listener]; !ok {
		return false
	}
	m := r.announced[listener]
	if m == nil {
		m = map[SessionID]struct{}{}
		r.announced[listener] = m
	}
	if _, ok := m[speaker]; ok {
		return false
	}
	m[speaker] = struct{}{}
	return true
}

// Subscribe 用一次**全量**声明整体替换该会话的订阅集合。
//
// 这是消除 sync 风暴的根本机制（spec 5.1）：幂等，没有"这次是加还是减"
// 的状态推导，重连后重发一次即恢复。刻意不提供任何增量入口。
//
// 超限时按客户端在 Sub.RX / Sub.TX 里声明的先后顺序接受，超出部分依次拒绝——
// 这是客户端可见的契约（见 TestWhichFrequenciesSurviveALimitIsDeterministic），
// 不是"随便留哪几个都行"：客户端要拿 ACK 对账，接受集合如果不确定，
// 每次重连就会落在不同的频率上。
//
// 声明本身还有一道与限额无关的上界（declarationLimit）：超出的部分在**拿锁
// 之前**就被截掉，并和其它被拒的频率一起回报。它挡的不是"能订阅几个"，而是
// "服务端愿意为一句声明在全局写锁里干多少活"。
//
// 未知会话返回空 ACK。控制面保证 SUB 只会在会话存在时到达，所以这里
// 只是防御；调用方不应当靠 ACK 的内容去判断会话在不在。
func (r *Router) Subscribe(id SessionID, sub control.Sub) control.SubAck {
	// 截断在**拿写锁之前**。这一步不是整理，是这条路径上唯一挡住"客户端声明
	// 多少、服务端就在全局写锁里干多少活"的东西。
	//
	// 实测：一份塞满的 SUB 装得下 9356 个频率（65533 字节），一次 Subscribe
	// 因此持 r.mu 长达 280 微秒，而同一时间 50 次 Listeners() 从 11 微秒被推到
	// 67 毫秒——**6039 倍**。r.mu 是全服务端一把锁，所以那不是"这个客户端自己
	// 慢一点"，是全网的扇出排队等一条会话的一句声明。
	//
	// 上界本来就隐含在 MaxTX/MaxRX 里（超出的必然被拒），但那个隐含上界是
	// **判完之后**才成立的，而判本身要给整份列表建去重 map、逐个走闸门。
	// maxXCPairs 的注释（下面）逐字论证过同一件事，只是当时只落实到了 XC 上。
	limit, ok := r.declarationLimit(id)
	if !ok {
		return control.SubAck{RX: []uint32{}, TX: []uint32{}, Rejected: []uint32{}, RejectedXC: [][2]uint32{}}
	}
	// 去重和排序都在截断**之后**做：反过来的话，为了截断先得把整份列表过一遍，
	// 那正是要避免的工作量。
	var excessTX, excessRX []uint32
	var unreportedTX, unreportedRX int
	sub.TX, excessTX, unreportedTX = truncateDeclaration(sub.TX, limit)
	sub.RX, excessRX, unreportedRX = truncateDeclaration(sub.RX, limit)

	ack, unreported := r.subscribeLocked(id, sub, excessTX, excessRX, time.Now())
	unreported += unreportedTX + unreportedRX
	ack.RejectedTruncated = unreported > 0

	// 日志在**锁外**。这条路径持的是全服务端那一把写锁，而 stderr 是会卡的
	// （docker 的 json-file 驱动、journald 背压），在锁里记日志等于让全网的
	// 扇出排队等一次写日志。Add 的顶号日志出于同一个理由也在锁外。
	if unreported > 0 {
		slog.Info("a subscription's rejection list did not fit and was truncated",
			"session", id, "reported", len(ack.Rejected), "unreported", unreported)
	}
	return ack
}

// subscribeLocked 是 Subscribe 持锁的那一段，返回 ACK 和**没能回报出去的**
// 被拒频率个数。
//
// 拆出来不是为了好看：调用方要在锁外记一条日志，而 `defer r.mu.Unlock()` 会
// 让函数体里任何一句日志都落在锁内。把锁的范围变成一个函数，是这里唯一一种
// 不依赖"defer 是 LIFO、所以这两条的注册顺序有讲究"这类细节的写法。
func (r *Router) subscribeLocked(id SessionID, sub control.Sub, excessTX, excessRX []uint32, now time.Time) (control.SubAck, int) {
	r.mu.Lock()
	defer r.mu.Unlock()
	snap, degraded := r.positionsLocked()

	ack := control.SubAck{RX: []uint32{}, TX: []uint32{}, Rejected: []uint32{}, RejectedXC: [][2]uint32{}}
	s, ok := r.sessions[id]
	if !ok {
		return ack, 0
	}

	// 先把旧的索引全部摘掉，再按新声明重建。
	old := s.subs.Load()
	for f := range old.rx {
		r.unindex(f, id)
	}
	r.bumpXC(old.xc, -1)

	next := emptySubs()
	txLimit := s.MaxTX
	if (s.Role == "pilot" || s.Role == "observer") && txLimit > 1 {
		txLimit = 1
	}
	for _, f := range dedup(sub.TX) {
		if !s.allowsTX(f, snap, degraded, now) {
			ack.Rejected = append(ack.Rejected, f)
			continue
		}
		// TX 超出 token 里的 max_tx 就拒绝。权威值是 token，
		// 控制面 READY 里的同名字段只是回显（spec 6）。
		if len(next.tx) >= txLimit {
			ack.Rejected = append(ack.Rejected, f)
			continue
		}
		next.tx[f] = struct{}{}
		ack.TX = append(ack.TX, f)
	}

	// TX ⊆ RX：没有"只发不收"的电台（spec 9.1 的无线电栈耦合规则）。
	for f := range next.tx {
		next.rx[f] = struct{}{}
	}
	for _, f := range dedup(sub.RX) {
		if _, already := next.rx[f]; !already && len(next.rx) >= s.MaxRX {
			// 超出配置的 RX 上限就拒绝。已经因为 TX⊆RX 进来的频率不算
			// 新增，否则一个 max_tx 大于 max_rx 的 token 会把自己的 TX 挤掉。
			ack.Rejected = append(ack.Rejected, f)
			continue
		}
		next.rx[f] = struct{}{}
	}
	// 交叉耦合的校验与规范化。必须在 next.tx 建好之后做——normaliseXC 拿
	// **授权后**的 TX 集合当权限依据。
	var rejectedXC [][2]uint32
	next.xc, rejectedXC = normaliseXC(sub.XC, next.tx)
	ack.RejectedXC = append(ack.RejectedXC, rejectedXC...)

	s.subs.Store(next)

	for f := range next.rx {
		r.index(f, id)
		ack.RX = append(ack.RX, f)
	}
	r.bumpXC(next.xc, +1)
	// 被上界截掉的那一截也要**回报**，不能静默丢弃——这是本协议在别处反复申明
	// 的规矩（SubAck.RejectedXC 存在的全部理由）。
	//
	// 排在逐条判掉的那些**后面**，不是随便放的：这两截的数值分布不一样（截掉的
	// 是声明尾巴上的那一段），所以先加哪一截决定了"先截断后排序"这个错误实现
	// 会留下什么。放在后面，两种实现的结果才不同，
	// TestAMaximalSubStillProducesASendableAck 才分辨得出它们。
	ack.Rejected = append(ack.Rejected, excessTX...)
	ack.Rejected = append(ack.Rejected, excessRX...)

	// Rejected 里可能有重复：同一个频率可以先被 TX 限额拒、又被 RX 限额拒
	// （比如 max_tx=1 且 max_rx=1 时的第二个频率）。这里只去重 Rejected 自身——
	// 一个频率同时出现在 RX 和 Rejected 里是有意义的（TX 被拒但 RX 给了），
	// 不受这次去重影响。
	ack.Rejected = dedup(ack.Rejected)

	// ACK 必须有稳定顺序：ack.RX 是从 map 里遍历出来的，而 Go 的 map 遍历
	// 是随机的。客户端要拿 ACK 和自己的声明对账，顺序不稳会让对账间歇性翻车。
	slices.Sort(ack.RX)
	slices.Sort(ack.TX)
	slices.Sort(ack.Rejected)
	// 截断在排序之后，所以留下的是确定的那一批（数值最小的 maxRejected 个），
	// 而不是"碰巧先算出来的"。
	unreported := 0
	if len(ack.Rejected) > maxRejected {
		unreported = len(ack.Rejected) - maxRejected
		ack.Rejected = ack.Rejected[:maxRejected]
	}
	return ack, unreported
}

// Listeners 返回订阅了该频率的会话。
//
// 返回的 *Session 在锁外被使用，所以只能读它的不可变字段和 subs 原子指针。
func (r *Router) Listeners(freq uint32) []*Session {
	r.mu.RLock()
	defer r.mu.RUnlock()
	ids := r.rx[freq]
	out := make([]*Session, 0, len(ids))
	for id := range ids {
		if s, ok := r.sessions[id]; ok {
			out = append(out, s)
		}
	}
	return out
}

// MayTransmit 报告该会话有没有声明在这个频率上发送。
// 这是上行包的第一道校验：没声明就丢弃，否则任何人都能往任意频率喊话。
func (r *Router) MayTransmit(id SessionID, freq uint32) bool {
	snap, degraded := r.positions()
	r.mu.RLock()
	s, ok := r.sessions[id]
	r.mu.RUnlock()
	if !ok {
		return false
	}
	_, ok = s.subs.Load().tx[freq]
	return ok && s.allowsTX(freq, snap, degraded, time.Now())
}

// SessionCount 返回当前登记的会话数。
//
// 存在的理由是"被拒的握手有没有留下一条会话"这个问题从外面问不出来：被拒的
// 那一条永远拿不到 id，所以只能拿别的 id 去猜——而猜出来的断言恰好在整包一起
// 跑的时候永远成立（newSessionID 是进程全局递增、从不重置的）。数一下会话数
// 是那个问题的直接说法。
func (r *Router) SessionCount() int {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return len(r.sessions)
}

// Get 取一条会话。
func (r *Router) Get(id SessionID) (*Session, bool) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	s, ok := r.sessions[id]
	return s, ok
}

func (r *Router) index(freq uint32, id SessionID) {
	if r.rx[freq] == nil {
		r.rx[freq] = map[SessionID]struct{}{}
	}
	r.rx[freq][id] = struct{}{}
}

func (r *Router) unindex(freq uint32, id SessionID) {
	if m, ok := r.rx[freq]; ok {
		delete(m, id)
		if len(m) == 0 {
			delete(r.rx, freq)
		}
	}
}

// declaredSlack 是"允许客户端声明的频率数"相对"可能被接受的频率数"的倍数。
//
// 上界取 slack × max(MaxTX, MaxRX) 而不是恰好 max(MaxTX, MaxRX)，是因为声明
// 里合法地存在接受不了的东西：重复项不占名额（见 TestSubscribeDeduplicates-
// Frequencies），而且契约本来就允许客户端多声明几个、由服务端按声明顺序挑
// （TestWhichFrequenciesSurviveALimitIsDeterministic 钉的就是这个）。给 4 倍
// 是为了不误伤这类正当声明；真正要挡的是那种差三个数量级的——默认配置
// MaxRX=32 下这个上界是 128，而实测的最坏输入是 9356。
//
// 4 这个数字本身不承重：往上调到 8 或往下调到 2 都不会改变任何一条正当声明的
// 结果，只会改变"从多荒唐开始被截断"。承重的是**存在一个与客户端声明无关的
// 上界**。
const declaredSlack = 4

// declarationLimit 取该会话允许声明的频率数上界。
//
// 单独一次读锁，而不是在写锁里顺手读：整件事的意义就在于截断发生在写锁**之前**。
// 会话在这之后、拿到写锁之前消失是可能的，所以写锁里还会再查一次——那一次才是
// 权威的，这一次只用来取两个不可变字段。
func (r *Router) declarationLimit(id SessionID) (int, bool) {
	r.mu.RLock()
	s, ok := r.sessions[id]
	r.mu.RUnlock()
	if !ok {
		return 0, false
	}
	n := max(s.MaxTX, s.MaxRX)
	if n < 0 {
		n = 0
	}
	return n * declaredSlack, true
}

// truncateDeclaration 把一份声明截到 limit，返回**有界的**超出部分，以及
// 连那一截都装不下、因此谁也不会知道的个数。
//
// 超出部分只回报最多 maxRejected 个，理由和 normaliseXC 的上界分支一字不差：
// SubAck 要经 control.WriteFrame 发出去，那里是 64 KiB 的上限，原样抄回九千个
// 频率会让 ACK 超限而根本发不出去——客户端于是什么都收不到，比截断更糟。
// 多抄的部分反正也会被末尾的 maxRejected 砍掉，抄它只是白干一趟 O(n) 的活，
// 而这个函数存在的全部理由就是不干那趟活。
//
// 第三个返回值是那趟活的**账**。截断本身是对的，"截断了却不说"不是：实测声明
// 1000 个频率、MaxRX=32 时有 712 条拒绝无标记、无日志地消失，而客户端拿到的
// ACK 看起来完全正常。数出来交给调用方，它负责在 ACK 上立一个标记、在日志里
// 写一行。
func truncateDeclaration(xs []uint32, limit int) (kept, excess []uint32, unreported int) {
	if len(xs) <= limit {
		return xs, nil, 0
	}
	end := min(len(xs), limit+maxRejected)
	return xs[:limit], xs[limit:end], len(xs) - end
}

// maxXCPairs 是一份声明里最多处理多少个耦合对。
//
// 超出的直接拒。上界本来就由 TX 授权集合隐含（两个频率都必须在里面，所以最多
// C(MaxTX,2) 对），但那个隐含上界是**检查完之后**才成立的，而检查本身要遍历
// 客户端给的整份列表并往 rejected 里抄。控制帧上限是 64 KiB（control.MaxFrame），
// 一份塞满的 SUB 能带七千多个对，所以"反正最后都会被拒"不是不设界的理由。
const maxXCPairs = 64

// maxRejected 是 SubAck.Rejected 里最多回报多少个频率。
//
// 不设界的话 ACK 会超过出站帧上限而根本发不出去——一条**已经生效**的 SUB
// 得不到任何回应。实测的最坏输入：贪心地用短数字塞满一条 SUB，12768 个互不相同
// 的 TX 频率正好是 65535 字节（`control.MaxFrame` 是 65536），而回来的 SubAck 是
// 65575 字节，超了 39 个。放大倍数约等于 1 加上几十字节的 JSON 键名，所以这不是
// 一个能拿去打别人的放大器，但它是自伤的、活的。
//
// 截断是安全的，理由值得写下来：**客户端随时可以自己算出被拒的集合**——它自己
// 的声明减去 ack.RX ∪ ack.TX 就是。Rejected 是给"拒了三两个"这种常见情况用的
// 便利字段，不是权威记录。声明了几千个频率却只拿回一截列表的客户端没有受害；
// 什么 ACK 都没拿到的客户端才受害。
//
// 256 的来历（算出来的，不是拍的）：频率是 uint32，十进制最长 10 位加一个逗号，
// 按 11 字节算；RejectedXC 最坏 2*maxXCPairs = 128 对，每对最长 23 字节。于是固定
// 部分约 64（键名）+ 128*23 + 256*11 = 5824 字节，给 ack.RX + ack.TX 留下约 59700
// 字节、合 5400 个频率。RX/TX 的长度由 MaxRX/MaxTX 决定，是服务端配置（通常
// 32/8），离 5400 有三个数量级的余量。真要把 MaxRX 配到几千，这个上界要重算。
const maxRejected = 256

// normaliseXC 校验并规范化客户端声明的交叉耦合对，返回生效的对和被拒的对。
//
// 三件事，顺序有讲究：
//
//  1. **两个频率都必须在该会话的 TX 授权集合里。** 这是权限检查，不是整理。
//     耦合是全服务端生效的（见 coupledWith），所以不校验的话，任何一条会话都能
//     声明 XC: [[121800, 123450]] 把两个不相干的频率接通——而它可能这两个频率
//     一个都没有。这和无线电栈本来的耦合规则一致：打开 XC 会强制 RX 和 TX 都
//     打开，也就是说你只能把自己正在发射的频率接起来。
//     用**授权后**的 tx 集合而不是客户端声明的 sub.TX：被 MaxTX 拒掉的频率不算
//     你的，否则限额就绕开了。
//  2. 丢掉自耦合（{f,f}）。它没有意义，而且会让同一个听众在目标频率列表里
//     出现两次。
//  3. 每对内部升序，然后整体去重。**这两件事都是纵深防御，不是承重件，
//     不要给它们编一个它们并不承担的理由。** 实测过：把升序去掉、把去重去掉，
//     整套 router 测试各自仍然全绿。原因是 bumpXC 对每一对都同时动两个方向，
//     所以对的朝向根本到不了索引里；而加走 next.xc、减走 old.xc，是同一份列表，
//     写三遍就加三遍减三遍，照样归零。coupledWith 又是遍历 map 的，重复只会让
//     某个键的计数变大，不会让它返回重复的频率。
//     它们真正买到的是：索引里一个无向对只占一个键，计数的含义是"有几条会话
//     声明了这一对"而不是"一共写了几个字"，于是 subs.xc 和计数都不会被一份
//     写了 64 遍同一对的声明撑大。（"同一对写三遍会让计数加三、一次撤销只减一，
//     于是永远撤不掉"是不成立的——撤销减的是存下来的那一整份列表。）
//
// 顺带这也完成了"复制而不是引用调用方的切片"：[2]uint32 是值类型，append 到一个
// 新切片里就是按值拷贝，调用方之后改自己的缓冲区不会动到路由状态。
func normaliseXC(pairs [][2]uint32, tx map[uint32]struct{}) (ok, rejected [][2]uint32) {
	seen := make(map[[2]uint32]struct{}, min(len(pairs), maxXCPairs))
	for i, p := range pairs {
		if i >= maxXCPairs {
			// 超出上界的一律拒，而且要**告诉**客户端——静默丢弃正是这条规则
			// 要避免的失败。但回报本身也必须有界：SubAck 要经 control.WriteFrame
			// 发出去，那里同样是 64 KiB 的上限，原样抄回七千个对会让 ACK 超限
			// 而根本发不出去，于是客户端什么都收不到——比静默丢弃更糟。
			// 最坏情况因此是 maxXCPairs（逐条判掉的）+ maxXCPairs（这里的）。
			rejected = append(rejected, pairs[i:min(len(pairs), i+maxXCPairs)]...)
			break
		}
		if p[0] == p[1] {
			rejected = append(rejected, p)
			continue
		}
		if _, has := tx[p[0]]; !has {
			rejected = append(rejected, p)
			continue
		}
		if _, has := tx[p[1]]; !has {
			rejected = append(rejected, p)
			continue
		}
		if p[0] > p[1] {
			p[0], p[1] = p[1], p[0]
		}
		if _, dup := seen[p]; dup {
			continue // 重复不算被拒，客户端声明的那一对确实生效了
		}
		seen[p] = struct{}{}
		ok = append(ok, p)
	}
	return ok, rejected
}

// bumpXC 把一组耦合对加进索引（delta = +1）或摘出来（delta = -1）。
// 对是双向的，所以两个方向都要动。
func (r *Router) bumpXC(pairs [][2]uint32, delta int) {
	for _, p := range pairs {
		r.bumpOneXC(p[0], p[1], delta)
		r.bumpOneXC(p[1], p[0], delta)
	}
}

func (r *Router) bumpOneXC(from, to uint32, delta int) {
	m := r.xc[from]
	if m == nil {
		if delta < 0 {
			return
		}
		m = map[uint32]int{}
		r.xc[from] = m
	}
	m[to] += delta
	if m[to] <= 0 {
		delete(m, to)
	}
	if len(m) == 0 {
		delete(r.xc, from)
	}
}

func dedup(xs []uint32) []uint32 {
	seen := make(map[uint32]struct{}, len(xs))
	out := make([]uint32, 0, len(xs))
	for _, x := range xs {
		if _, ok := seen[x]; ok {
			continue
		}
		seen[x] = struct{}{}
		out = append(out, x)
	}
	return out
}
