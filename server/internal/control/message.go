package control

import (
	"encoding/json"
	"fmt"
	"unicode/utf8"
)

// ProtoVersion 是 Hello.Proto 里唯一被接受的值。
//
// 它和 ALPN（`can-voice/1`）说的是同一件事的两层：ALPN 在 TLS 握手时就把版本
// 对上了，所以正常情况下走不到这里——但 ALPN 只约束"这条连接讲的是哪个协议"，
// 而 Proto 是**客户端自己声明的控制面版本**，两者可以不一致（一个把 ALPN 抄对、
// 却按旧版控制面编消息的客户端就是）。
//
// 服务端必须真的看这个字段。声明了却从不读的字段是最坏的一种：客户端作者照着
// 它填，以为填错了会被告知，而实际上 `"proto": 99999` 照样拿到 READY，然后在
// 第一条它看不懂的消息上出问题——那时候已经没有任何东西指向版本了。
const ProtoVersion = 1

// Hello 是客户端的第一条消息。Follow 只有观察员模式用——
// 观察员没有 FSD 连接，位置取自它跟随的那架飞机（spec 7.3）。
//
// Station 是同一个账号下的席位标记，只有通播机队这种"整队共用一个 CID"的
// 客户端填。顶号按 (CID, Station) 判，所以不填的普通客户端行为和以前一样：
// 同一个成员号第二次登录，第一条会话被断开。
type Hello struct {
	Type    string `json:"type"`
	Token   string `json:"token"`
	Client  string `json:"client"`
	Proto   int    `json:"proto"`
	Follow  string `json:"follow,omitempty"`
	Station string `json:"station,omitempty"`
}

// Ready 是服务端对 Hello 的回应。
// MaxTX 在这里只是回显，权威值在 token 里（spec 6）。
type Ready struct {
	Type    string `json:"type"`
	Session uint32 `json:"session"`
	Server  string `json:"server"`
	MaxTX   int    `json:"max_tx"`
	MaxRX   int    `json:"max_rx"`
}

// Sub 是**全量**收发声明，不是增量。服务端收到即整体替换该会话的订阅集合。
// 这是消除 sync 风暴的根本机制：幂等，无状态推导，重连后重发一次即恢复。
// 刻意没有 add/remove 字段——任何增量语义都会把那一类 bug 请回来。
type Sub struct {
	Type string      `json:"type"`
	RX   []uint32    `json:"rx"`
	TX   []uint32    `json:"tx"`
	XC   [][2]uint32 `json:"xc"`
}

// SubAck 回显服务端实际接受的集合，并列出被拒的频率。
//
// Rejected 是没有按声明给到的频率。一个频率可以同时出现在 RX 和
// Rejected 里：那表示 TX 被限额拒了，但 RX 给了。
type SubAck struct {
	Type     string   `json:"type"`
	RX       []uint32 `json:"rx"`
	TX       []uint32 `json:"tx"`
	Rejected []uint32 `json:"rejected"`
	// RejectedXC 是没有生效的交叉耦合对。
	//
	// 一对耦合只有在该会话的 TX 授权集合里同时包含两个频率时才生效——耦合是
	// 全服务端生效的，不校验的话任何人都能把两个不相干的频率接通。被拒的对
	// 必须回报：一个设好了交叉耦合却不生效、又不知道为什么的管制员，比一个
	// 被明确拒绝的管制员糟糕得多。
	RejectedXC [][2]uint32 `json:"rejected_xc"`

	// RejectedTruncated 说的是"Rejected 这张单子本身不全"。
	//
	// Rejected 有上界（maxRejected），声明本身也有上界（router 的
	// declarationLimit），两道上界都是必要的——不设界的 ACK 会超过 64 KiB 的
	// 帧上限而根本发不出去，那比截断糟得多。但**截断了却不说**是这套协议在
	// 别处反复拒绝的那种失败：实测声明 1000 个频率、MaxRX=32 时，有 712 条
	// 拒绝无标记、无日志地消失，而客户端拿到的 ACK 看起来完全正常。
	//
	// 客户端该怎么用它：为 true 时不要拿 Rejected 当权威记录，改用差集
	// `声明 − (ack.RX ∪ ack.TX)` 自己算——那个式子任何时候都成立，Rejected
	// 只是"拒了三两个"这种常见情况下的便利字段。
	//
	// omitempty 是有意的：它只在真发生时出现在线上，所以常规 SUBACK 的字节数
	// 一个都没变（maxRejected 那段 64 KiB 的算术依赖 ACK 的尺寸）。
	RejectedTruncated bool `json:"rejected_truncated,omitempty"`
}

// Notice 是服务端的单向通知，Kind 取值见 KindTxDenied 等常量。
type Notice struct {
	Type   string `json:"type"`
	Kind   string `json:"kind"`
	Freq   uint32 `json:"freq,omitempty"`
	Reason string `json:"reason,omitempty"`
	// Session / CID 只在 KindTalker 上有：数据面包头的 speaker 是会话号，
	// 电台行要显示的是 CAN 号（再经 datafeed 翻成呼号）。旧客户端不认识这两
	// 个键，serde / encoding/json 都会忽略。
	Session uint32 `json:"session,omitempty"`
	CID     string `json:"cid,omitempty"`
}

// Notice 的 Kind 取值。
const (
	KindTxDenied         = "tx_denied"
	KindRangeUnavailable = "range_unavailable"
	// KindTalker：某个会话开始对这个听众说话。每个听众对每个发言者只发一次。
	KindTalker = "talker"
	// **没有 sub_rejected，而且不该有。** 被拒的订阅走 SUBACK 的 Rejected /
	// RejectedXC 两张单子，那是 SUB 的同步答复，客户端按差集分派（见 Rust 侧的
	// on_ack）。再发一条 NOTICE 是把同一件事在同一条流上说两遍，而两份报告一旦
	// 不一致就没有哪一份可信。这个常量曾经存在、从没被发出过，删掉的理由就是
	// 这个——一个宣称了未实现行为的字段比没有这个字段更糟。
	// KindUnknownMessage 是"你发来的这一帧我解不开"。
	//
	// 服务端**不**因此断开——一个比它新一个协议版本的客户端应该降级，不该掉线，
	// 而且断开是不对称的：服务端没法解释为什么，在客户端看来就是掉线，于是它重连、
	// 重发，形成无限循环。但也不能就这么静默：客户端发了一条消息、什么都没发生、
	// 又不知道为什么，正是 Decode 对未知类型报错而不是静默忽略要躲开的那种失败
	// 形态，只是从服务端内部挪到了线上。先例就在本协议里——SubAck.RejectedXC 存在
	// 的全部理由就是"一个设好了交叉耦合却不生效、又不知道为什么的管制员，比一个
	// 被明确拒绝的管制员糟糕得多"。同一条原则。
	KindUnknownMessage = "unknown_message"
	// KindRateLimited 是"你的上行超过了正常语音的速率，多出来的被丢掉了"。
	//
	// 和 KindUnknownMessage 一样**不断开**，理由也一样：触发它的几乎一定是客户端
	// 的一个 bug（PTT 卡住之后发包循环失控），而断开在对端看来只是掉线，于是它
	// 重连、重放，回到同一个地方。丢掉多出来的那些、留着会话，是唯一能收敛的处理。
	//
	// 也不能静默：被限住的那个人说了话、没人听见、而他那边的界面一切正常——
	// 这正是 tx_denied 存在要躲开的那种失败形态，只是原因换了一个。
	// 频率不带：桶是按**会话**算的（见 transport/uplink.go），一次超额不归某一个
	// 频率，填一个进去等于替客户端指认一个不存在的元凶。
	KindRateLimited = "rate_limited"
)

// MaxTypeLen 是 TypeOf 返回的类型字符串上限。
//
// 有上限，是因为这个字符串会被原样回给对端（放进 NOTICE 的 Reason）并写进日志，
// 而它的内容完全由对端决定：一个 64 KB 的 type 字段（MaxFrame 允许）不设限就会
// 变成一条 64 KB 的 NOTICE 和一行 64 KB 的日志。真实的类型名都是十来个字符。
const MaxTypeLen = 32

// TypeOf 只取出一个控制帧的 type 判别字段，不解码其余部分。
//
// 给"解不开的帧"那条路径用：要告诉对端**是哪一个类型**没被认出来，而不是把它
// 发来的原始报文原样回显——报文可能很长，回显它既没必要也没好处。帧根本不是
// JSON、或者没有 type 字段时返回空串，调用方自己决定那时候说什么。
func TypeOf(b []byte) string {
	var probe struct {
		Type string `json:"type"`
	}
	if err := json.Unmarshal(b, &probe); err != nil {
		return ""
	}
	if len(probe.Type) <= MaxTypeLen {
		return probe.Type
	}
	// 按 rune 边界截断：从中间切断一个多字节字符会留下非法 UTF-8，
	// json.Marshal 会把它换成 U+FFFD，读日志的人只会更糊涂。
	cut := MaxTypeLen
	for cut > 0 && !utf8.RuneStart(probe.Type[cut]) {
		cut--
	}
	return probe.Type[:cut]
}

// Ping/Pong 只用来测 RTT；保活由 QUIC 自己做。
type Ping struct {
	Type string `json:"type"`
	T    int64  `json:"t"`
}

type Pong struct {
	Type    string `json:"type"`
	T       int64  `json:"t"`
	ServerT int64  `json:"server_t"`
}

// Bye 是服务端主动断开前的最后一条消息。
type Bye struct {
	Type   string `json:"type"`
	Reason string `json:"reason"`
}

// Encode 把消息编成 JSON，并填好 Type 判别字段。
func Encode(m any) ([]byte, error) {
	switch v := m.(type) {
	case *Hello:
		v.Type = "HELLO"
	case *Ready:
		v.Type = "READY"
	case *Sub:
		v.Type = "SUB"
	case *SubAck:
		v.Type = "SUBACK"
	case *Notice:
		v.Type = "NOTICE"
	case *Ping:
		v.Type = "PING"
	case *Pong:
		v.Type = "PONG"
	case *Bye:
		v.Type = "BYE"
	default:
		return nil, fmt.Errorf("cannot encode %T as a control message", m)
	}
	return json.Marshal(m)
}

// Decode 按 type 字段分发。未知类型直接拒绝，不静默忽略——
// 静默忽略会让一个拼错的类型（或者一个更新协议版本发来的、这个 build
// 从未听说过的消息类型）表现为"消息发出去了但什么都没发生"，
// 这跟 SUB 必须是全量声明而不是增量是同一种纪律：宁可让调用方显式地
// 处理一个错误，也不要让协议的一部分悄悄地被吞掉。
func Decode(b []byte) (any, error) {
	var probe struct {
		Type string `json:"type"`
	}
	if err := json.Unmarshal(b, &probe); err != nil {
		return nil, fmt.Errorf("control frame is not valid JSON: %w", err)
	}
	var m any
	switch probe.Type {
	case "HELLO":
		m = &Hello{}
	case "READY":
		m = &Ready{}
	case "SUB":
		m = &Sub{}
	case "SUBACK":
		m = &SubAck{}
	case "NOTICE":
		m = &Notice{}
	case "PING":
		m = &Ping{}
	case "PONG":
		m = &Pong{}
	case "BYE":
		m = &Bye{}
	default:
		return nil, fmt.Errorf("unknown control message type %q", probe.Type)
	}
	if err := json.Unmarshal(b, m); err != nil {
		return nil, fmt.Errorf("decode %s: %w", probe.Type, err)
	}
	return m, nil
}
