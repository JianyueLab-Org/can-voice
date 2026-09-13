package control

import (
	"encoding/json"
	"fmt"
	"unicode/utf8"
)

// Hello 是客户端的第一条消息。Follow 只有观察员模式用——
// 观察员没有 FSD 连接，位置取自它跟随的那架飞机（spec 7.3）。
type Hello struct {
	Type   string `json:"type"`
	Token  string `json:"token"`
	Client string `json:"client"`
	Proto  int    `json:"proto"`
	Follow string `json:"follow,omitempty"`
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
}

// Notice 是服务端的单向通知，Kind 取值见 KindTxDenied 等常量。
type Notice struct {
	Type   string `json:"type"`
	Kind   string `json:"kind"`
	Freq   uint32 `json:"freq,omitempty"`
	Reason string `json:"reason,omitempty"`
}

// Notice 的 Kind 取值。
const (
	KindTxDenied         = "tx_denied"
	KindRangeUnavailable = "range_unavailable"
	KindSubRejected      = "sub_rejected"
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
