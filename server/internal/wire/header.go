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

// Header 是数据面包头。
//
// Qual 与 Speaker 上行时由客户端填 0，由服务端填入后再扇出：
// 客户端因此不知道任何人的位置，而 Speaker 让接收端能分辨
// "同一频率上有两个人在讲"与"一个人的包乱序了"。
type Header struct {
	Ver     uint8
	Flags   uint8
	Qual    uint8
	Seq     uint16
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
