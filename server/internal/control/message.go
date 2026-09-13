package control

import (
	"encoding/json"
	"fmt"
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
type SubAck struct {
	Type     string   `json:"type"`
	RX       []uint32 `json:"rx"`
	TX       []uint32 `json:"tx"`
	Rejected []uint32 `json:"rejected"`
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
)

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
