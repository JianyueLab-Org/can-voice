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

	"github.com/JianyueLab-Org/can-voice/server/internal/control"
)

// Router 持有全部服务端状态。
type Router struct {
	mu       sync.RWMutex
	sessions map[SessionID]*Session
	// rx 是频率到订阅者的倒排索引，扇出时只查这一张表。
	rx map[uint32]map[SessionID]struct{}
	// byCID 是成员号到当前会话的索引，只为顶号服务。
	// 一个 CID 最多一条会话，所以是 SessionID 而不是集合。
	byCID map[string]SessionID
}

// New 建一个空的 Router。
func New() *Router {
	return &Router{
		sessions: map[SessionID]*Session{},
		rx:       map[uint32]map[SessionID]struct{}{},
		byCID:    map[string]SessionID{},
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
	// Follow 只有观察员模式填：观察员没有 FSD 连接，
	// 位置取自它跟随的那架飞机（spec 7.3）。
	Follow string
	// MaxTX 来自 token：最多能在几个频率上发送。
	MaxTX int
	// MaxRX 来自服务端配置：最多能订阅几个频率。
	MaxRX int
	// Send 把一个数据面包发给这个会话。
	Send func([]byte)
	// Close 断开底层连接。可以为 nil（纯逻辑测试里就是）。
	Close func()
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
	s := &Session{
		ID:        newSessionID(),
		CID:       o.CID,
		Follow:    o.Follow,
		MaxTX:     o.MaxTX,
		MaxRX:     o.MaxRX,
		send:      o.Send,
		closeConn: o.Close,
	}
	s.subs.Store(emptySubs())

	r.mu.Lock()
	var evicted *Session
	if o.CID != "" {
		// 空 CID 不参与顶号。它不该出现（鉴权拒绝空 CID），但真出现时让所有
		// 空 CID 会话互相顶掉，比放过去糟糕得多。
		if prev, ok := r.byCID[o.CID]; ok {
			evicted = r.sessions[prev]
			r.removeLocked(prev)
		}
		r.byCID[o.CID] = s.ID
	}
	r.sessions[s.ID] = s
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
	for f := range s.subs.Load().rx {
		r.unindex(f, id)
	}
	s.subs.Store(emptySubs())
	// cid 索引也要清，否则留下一个指向已注销会话的条目，下一次同 cid 登录
	// 会对着它调 Close，而那个闭包捕获的是一条已经关掉的连接。
	// 只在它确实指向这条会话时才删——顶号路径上新条目稍后才写入，
	// 但别的顺序下这个判断是防止误删的那道闸。
	if cur, ok := r.byCID[s.CID]; ok && cur == id {
		delete(r.byCID, s.CID)
	}
	delete(r.sessions, id)
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
// 未知会话返回空 ACK。控制面保证 SUB 只会在会话存在时到达，所以这里
// 只是防御；调用方不应当靠 ACK 的内容去判断会话在不在。
func (r *Router) Subscribe(id SessionID, sub control.Sub) control.SubAck {
	r.mu.Lock()
	defer r.mu.Unlock()

	ack := control.SubAck{RX: []uint32{}, TX: []uint32{}, Rejected: []uint32{}}
	s, ok := r.sessions[id]
	if !ok {
		return ack
	}

	// 先把旧的索引全部摘掉，再按新声明重建。
	for f := range s.subs.Load().rx {
		r.unindex(f, id)
	}

	next := emptySubs()
	for _, f := range dedup(sub.TX) {
		// TX 超出 token 里的 max_tx 就拒绝。权威值是 token，
		// 控制面 READY 里的同名字段只是回显（spec 6）。
		if len(next.tx) >= s.MaxTX {
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
	// 复制而不是引用调用方的切片：控制面那边复用缓冲区就会改到路由状态。
	next.xc = append([][2]uint32(nil), sub.XC...)

	s.subs.Store(next)

	for f := range next.rx {
		r.index(f, id)
		ack.RX = append(ack.RX, f)
	}
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
	return ack
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
	r.mu.RLock()
	s, ok := r.sessions[id]
	r.mu.RUnlock()
	if !ok {
		return false
	}
	_, ok = s.subs.Load().tx[freq]
	return ok
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
