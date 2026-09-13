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
}

// New 建一个空的 Router。
func New() *Router {
	return &Router{
		sessions: map[SessionID]*Session{},
		rx:       map[uint32]map[SessionID]struct{}{},
	}
}

// Add 登记一条新会话。send 会在扇出时被调用。
//
// 新会话带着一个空声明入场，不是 nil——扇出完全可能在第一个 SUB 到达之前
// 就看到它，那时 subs.Load() 必须已经可用。
//
// maxTX 来自 token（是鉴权的一部分），maxRX 来自服务端配置（是资源上限）。
// 两者来源不同，所以是两个参数而不是一个结构体。
func (r *Router) Add(cid, follow string, maxTX, maxRX int, send func([]byte)) *Session {
	s := &Session{
		ID:     newSessionID(),
		CID:    cid,
		Follow: follow,
		MaxTX:  maxTX,
		MaxRX:  maxRX,
		send:   send,
	}
	s.subs.Store(emptySubs())
	r.mu.Lock()
	defer r.mu.Unlock()
	r.sessions[s.ID] = s
	return s
}

// Remove 注销会话并把它从每一个频率的订阅者集合里摘掉。
func (r *Router) Remove(id SessionID) {
	r.mu.Lock()
	defer r.mu.Unlock()
	s, ok := r.sessions[id]
	if !ok {
		return
	}
	for f := range s.subs.Load().rx {
		r.unindex(f, id)
	}
	s.subs.Store(emptySubs())
	delete(r.sessions, id)
}

// Subscribe 用一次**全量**声明整体替换该会话的订阅集合。
//
// 这是消除 sync 风暴的根本机制（spec 5.1）：幂等，没有"这次是加还是减"
// 的状态推导，重连后重发一次即恢复。刻意不提供任何增量入口。
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
