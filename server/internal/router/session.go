package router

import "sync/atomic"

// SessionID 是一个连接的标识，也是数据面包头里的 speaker 字段。
// 从 1 开始，所以 0 永远不是一个有效会话——包头里的 0 可以当哨兵用。
type SessionID uint32

// subs 是一次 SUB 声明的产物。构造完成之后永不修改。
//
// 订阅状态做成"不可变值 + 原子整体替换"而不是"互斥锁保护的可变字段"，
// 是因为它会在 Router 的锁外被读到：MayTransmit 取完会话指针就放锁，然后才
// Load()；Listeners() 更是把 *Session 直接交给扇出路径。可变字段在那里就是
// 并发读写：读到撕裂的 map/切片头会把音频转发到随机频率，或者直接越界 panic。
//
// 这也正好是全量 SUB 的设计本身——订阅是一个整体声明，不是一串增量，
// 所以数据结构把设计原则表达出来了。
type subs struct {
	rx map[uint32]struct{}
	tx map[uint32]struct{}
	// xc 是这条会话声明的、且通过了校验的交叉耦合对，每对内部升序、整体去重
	// （见 normaliseXC）。扇出不读它——耦合是全服务端的，查的是 Router.xc 那张
	// 引用计数索引。留着它是为了知道下一次 Subscribe 或 Remove 该从索引里
	// 摘掉哪些对。
	xc [][2]uint32
}

func emptySubs() *subs {
	return &subs{rx: map[uint32]struct{}{}, tx: map[uint32]struct{}{}}
}

// Session 是一条连接。
//
// 刻意不持有任何"我在哪个频道"之类的可记忆状态——
// 订阅集合是服务端的唯一真相，由客户端每次用全量 SUB 声明。
//
// ID/CID/Follow/MaxTX/send 在 Add 里写定，此后不再修改，可以无锁读；
// 订阅状态只能经由 subs 这个原子指针读写。Session 含 atomic.Pointer，
// 不可拷贝——永远用 *Session（go vet 的 copylocks 会盯着这一点）。
type Session struct {
	ID  SessionID
	CID string
	// Follow 只有观察员模式填：观察员没有 FSD 连接，
	// 位置取自它跟随的那架飞机（spec 7.3）。
	Follow string
	// Station 是同一个账号下的第几个席位，空串表示"就一个"。见 evictionKey。
	Station string
	// MaxTX 来自 token：客户端最多能在几个频率上发送。
	MaxTX int
	// MaxRX 来自服务端配置：客户端最多能订阅几个纯 RX 频率。
	//
	// 这一层的真实上界不是 MaxRX 本身，而是 max(MaxTX, MaxRX)：TX 蕴含 RX
	// （spec 9.1 的耦合规则），且 TX 频率不受 RX 限额挤压——见
	// TestTxIsNotSqueezedOutByTheRxLimit，这是有意的设计，不是漏洞。
	//
	// 但**它也正是一条绕过路径**，所以传输层在握手时就把 MaxTX 夹到了 MaxRX
	// 以内（transport 的 grantedMaxTX）：不夹的话，一张 max_tx 很大的 token
	// 能让 max(MaxTX, MaxRX) 变成它想要的任何数，而 MaxRX 这个服务端配置就
	// 形同虚设。router 自己不夹——它是纯逻辑层，允许调用方构造任意组合，
	// 这两个字段的含义不该在这里被悄悄改写。
	//
	// MaxRX 为 0 时，纯 RX 声明会全部被拒，而 TX 蕴含进来的那些照常通过；
	// 这是配置校验的责任（Task 11 的 LoadConfig 拒绝非正的
	// CAN_VOICE_MAX_RX），这里不重复校验，见 TestZeroMaxRXStillAllowsTxImpliedRx。
	//
	// 必须真的强制，不能只在 READY 里通告一下。每个 RX 频率都要写进
	// Router 的倒排索引，而那是在写锁里做的——一个声明了一万个频率的
	// 会话会让全网的扇出排队等它。控制面握手之后的消息是已鉴权的，
	// 但"已鉴权"不等于"可信"。
	MaxRX int

	// send 把一个数据面包发给这个会话。由传输层注入，
	// router 因此不依赖 QUIC，可以纯逻辑测试。
	send func([]byte)

	// closeConn 断开这条会话的底层连接。同一个 CID 再次登录时用它顶掉旧会话。
	// 和 send 一样由传输层注入，router 因此仍然不认识 QUIC。
	//
	// 只在顶号时调用，正常的 Remove 不调用——那条路径上传输层本来就正在
	// 拆连接，回调进去等于让它自己拆自己。
	closeConn func()

	subs atomic.Pointer[subs]
}

// Send 把一个已编好的数据面包发出去。
func (s *Session) Send(b []byte) {
	if s.send != nil {
		s.send(b)
	}
}

var nextID atomic.Uint32

func newSessionID() SessionID {
	for {
		if id := SessionID(nextID.Add(1)); id != 0 {
			return id
		}
		// 回绕到 0。0 是包头里的哨兵值，不能发给任何人。
	}
}
