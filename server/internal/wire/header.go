// Package wire 是 can-voice 数据面的包头编解码。
//
// 这是一个跨实现契约：Rust 侧（can-voice-client）有一份独立实现，两边都测
// testdata/wire-golden.json。改这里的布局等于改协议，必须同时改黄金文件和 Rust 侧。
package wire

import (
	"encoding/binary"
	"fmt"
)

// HeaderSize 是数据面包头的字节数。布局见 spec 5.2：
//
//	0        1        2        3               5               9              13
//	+--------+--------+--------+---------------+---------------+--------------+
//	|  ver   | flags  |  qual  |    seq(2)     |  freq_khz(4)  | speaker(4)   | opus…
//	+--------+--------+--------+---------------+---------------+--------------+
const HeaderSize = 13

// Version 是本实现能处理的唯一协议版本。
const Version uint8 = 1

// flags 位。
const (
	// FlagFirst 标记一次发言的首帧：接收端据此立刻点亮 RX 指示灯并重置抖动缓冲。
	FlagFirst uint8 = 1 << 0
	// FlagLast 标记一次发言的尾帧：接收端据此立刻熄灭 RX 指示灯，
	// 而不是靠一个超时循环——那会让指示灯在松开 PTT 后多亮半秒。
	FlagLast uint8 = 1 << 1
)

// ReservedFlags 是 flags 里今天没有含义的那六位（第 2–7 位，也就是 0xFC）。
//
// **规则：发送方必须置零，接收方必须忽略自己不认识的位，服务端原样转发。**
// 服务端既不校验也不清零——这是一个决定而不是遗漏，三条路各自的后果值得写下来：
//
//   - 收到就拒：一个将来想用第 2 位的客户端必须先等服务端升级，还得挑一个
//     所有服务端都升级完的日子才能开这一位。而服务端对载荷本来就刻意不解码，
//     让它对 flags 有意见，等于给一个专门不懂音频的中继加一条懂音频的规矩。
//   - 静默清零：那一位就那么没了，两个客户端都以为自己在讲同一个方言，
//     没有任何一句话告诉它被改过——这正是本协议在别处（SubAck.RejectedXC、
//     未知类型的 NOTICE、ack_undeliverable）反复拒绝的那种失败形态。
//   - 保留并原样转发（现在这条）：将来的一位可以**只升级客户端**就部署，
//     服务端一行都不用改，而今天的客户端按"忽略不认识的位"照常工作。
//
// 于是"原样转发"是承重的，由 TestFanoutRelaysTheReservedFlagBitsUntouched 钉住：
// 哪天有人顺手在这里加一道清零或者校验，那条测试就红。
const ReservedFlags uint8 = 0xFC

// Header 是数据面包头。
//
// 每个字段的上下行契约（跨实现的部分，Rust 侧照这段写）：
//
//	Ver      上下行都必须是 Version；Parse 不认识的版本直接拒。
//	Flags    FlagFirst/FlagLast 由客户端填、服务端原样转发；第 2–7 位见 ReservedFlags。
//	Qual     **上行必须填 0 且服务端完全忽略它**；下行由服务端填，恒在 1–255。
//	Seq      客户端填，服务端原样转发，见 Seq 字段那一段。
//	FreqKHz  客户端填；服务端原样转发，交叉耦合时改写成实际投递的那个频率。
//	Speaker  **上行必须填 0**，服务端填成发言者的会话 id 之后再扇出。
//
// Qual 与 Speaker 上行填 0，是因为这两个值客户端说了不算：客户端因此不知道
// 任何人的位置，而 Speaker 让接收端能分辨"同一频率上有两个人在讲"与
// "一个人的包乱序了"。
type Header struct {
	Ver   uint8
	Flags uint8

	// Qual 是信号质量。**下行恒在 1–255，永远不会是 0。**
	//
	// 这条不变量由两处合起来给出，而它此前哪儿都没写下来：射程外
	// geo.Quality 返回 (0, false)，服务端于是**根本不投递**这一包；而落在
	// 衰减带最外侧、四舍五入本来会得到 0 的那一小段（比值在 1.0994 与 1.1
	// 之间）被它夹到 1，理由是"还在射程内就不能报 0"。
	//
	// 接收端可以依赖它：qual 为 0 的包不会从服务端来，那只可能是一份上行报文
	// 或者本地自己造的包。反过来说，**不要**把下行的 0 当成"最弱的信号"去做
	// 静音处理——那种包不存在，那条分支永远不会执行，也就永远不会被发现写错了。
	//
	// 两层各自钉住：geo 的 TestQualityNeverReportsZeroWhileStillInRange，
	// 以及 router 的 TestFanoutStampsOneAtTheVeryEdgeNotZero。
	Qual uint8

	// Seq 是**每会话**的音频帧序号，服务端原样转发、从不改写。
	//
	// 这一段是写给抖动缓冲区的，P3 的 Rust 客户端核心直接照它实现：
	//
	//   - **每会话单调，不按发言重置。** 每编出一个 20 毫秒音频帧就加一，
	//     一次发言结束到下一次开始之间不清零。按发言重置在不可靠数据报上是错的：
	//     首帧本来就可能丢，丢了之后接收端看到的是一个**倒退**的序号，
	//     和"一个很旧的乱序包"分不开，于是整段新发言会被当成过期的丢掉。
	//   - **一个音频帧只有一个序号。** 一次 PTT 同时发在多个频率上时，那几份
	//     拷贝带的是**同一个** Seq；服务端做交叉耦合扇出时也照抄，不按频率
	//     另起一个。所以接收端按 (Speaker, FreqKHz) 分流之后看到的是一串连号。
	//   - **按 (Speaker, FreqKHz) 建缓冲。** Seq 是发言者自己的计数器，
	//     两个发言者的序号空间毫无关系，服务端也不重新编号。
	//   - **比较要用 16 位回绕算术**（RFC 1982 那一套）：`(b-a) mod 2¹⁶ < 2¹⁵`
	//     就认为 b 在 a 之后。20 毫秒一帧时 uint16 每 65536 帧、约 21.8 分钟
	//     回绕一次，而那是**连续发话**的 21.8 分钟——直接比大小的实现会在那一刻
	//     把整条流卡住，直到序号重新追上来。
	//   - **发言之内的空档才是丢包。** FlagFirst 到 FlagLast 之间少了 n 个号就是
	//     丢了 n 帧；跨发言的跳变不是丢包（中间那些帧可能发在别的频率上），
	//     所以收到 FlagFirst 是重置缓冲，而不是去补空档。
	//
	// 服务端这一半（原样转发、耦合的每一份拷贝同号）由 router 的
	// TestFanoutRelaysTheSequenceNumberUntouchedOnEveryCoupledCopy 钉住。
	Seq uint16

	// FreqKHz 是频率，单位千赫，也就是 round(MHz × 1000)：121.800 → 121800。
	//
	// **服务端把它当成一个不透明的 32 位路由键：不做范围校验，也不给任何具体
	// 取值赋予含义。** 整个 uint32 都能用，包括 0 和 4294967295，没有哨兵值。
	// 邻近项目里确实有这种约定（can-audio 拿 199998 当"没设频率"的占位值，
	// 它的频道名是 FREQ_<六位千赫>），但那是**那边**的约定；can-voice 不知道它，
	// 也不会因此拒绝或者改写任何东西。
	//
	// 两条推论，都写给第二个实现：想把频率夹在 VHF 波段（118000–136975）里的
	// 客户端必须自己夹，服务端不会替它挡；而两个约好的实现可以拿波段外的值当
	// 私有频道用，服务端照样路由。
	FreqKHz uint32

	Speaker uint32
}

// AppendTo 把包头追加到 dst 并返回新切片。
func (h Header) AppendTo(dst []byte) []byte {
	var b [HeaderSize]byte
	b[0] = h.Ver
	b[1] = h.Flags
	b[2] = h.Qual
	binary.BigEndian.PutUint16(b[3:5], h.Seq)
	binary.BigEndian.PutUint32(b[5:9], h.FreqKHz)
	binary.BigEndian.PutUint32(b[9:13], h.Speaker)
	return append(dst, b[:]...)
}

// Parse 解出包头，并返回其后的 Opus 载荷（原切片的子切片，不复制）。
// 载荷可以为空——尾帧不必携带音频。
func Parse(b []byte) (Header, []byte, error) {
	if len(b) < HeaderSize {
		return Header{}, nil, fmt.Errorf("packet is %d bytes, need at least %d", len(b), HeaderSize)
	}
	h := Header{
		Ver:     b[0],
		Flags:   b[1],
		Qual:    b[2],
		Seq:     binary.BigEndian.Uint16(b[3:5]),
		FreqKHz: binary.BigEndian.Uint32(b[5:9]),
		Speaker: binary.BigEndian.Uint32(b[9:13]),
	}
	// 版本不认识就拒绝：默默接受等于把未来的布局当成现在的来解，
	// 那会表现为音频乱码而不是一条清晰的错误。
	if h.Ver != Version {
		return Header{}, nil, fmt.Errorf("unknown protocol version %d, this build speaks %d", h.Ver, Version)
	}
	return h, b[HeaderSize:], nil
}
