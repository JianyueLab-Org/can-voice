// Package control 是 can-voice 的控制面：长度前缀 JSON 帧。
//
// 控制面刻意用 JSON 而不是 protobuf：消息频率极低（登录、订阅变更、偶发通知），
// 可读性和排障便利压过字节效率——出问题时要能直接在日志里读懂发生了什么。
// 高频的音频走 internal/wire 的紧凑二进制，两个面严格分离：本包不导入 wire。
package control

import (
	"encoding/binary"
	"fmt"
	"io"
)

// MaxFrame 是一个控制帧的上限。SUB 会携带订阅列表，
// 一个管制员可能订阅几十个频率，2 字节长度前缀（64 KB）也够，
// 但没有理由把上限压到比数据面的 2 字节前缀更紧——用 4 字节前缀，留足余量。
const MaxFrame = 64 << 10

// WriteFrame 写一个 4 字节大端长度前缀加载荷。
func WriteFrame(w io.Writer, b []byte) error {
	if len(b) > MaxFrame {
		return fmt.Errorf("control frame of %d bytes exceeds the %d byte limit", len(b), MaxFrame)
	}
	var hdr [4]byte
	binary.BigEndian.PutUint32(hdr[:], uint32(len(b)))
	if _, err := w.Write(hdr[:]); err != nil {
		return err
	}
	_, err := w.Write(b)
	return err
}

// ReadFrame 读一个长度前缀帧。
//
// 长度检查在分配 payload 之前完成：一个恶意或损坏的 4 字节前缀
// （例如全 0xff）否则能让服务端去为一个不存在的帧分配到 4 GB。
//
// 干净的流结束与帧读到一半必须能区分开，调用方（后续任务的连接循环）
// 要能在日志里说清楚"对端正常挂断"和"对端在帧中间死了"是两回事：
//   - 上一帧读完后、下一帧还没开始时对端关闭连接：这是正常结束，
//     io.ReadFull 在长度前缀一个字节都没读到时天然返回 io.EOF。
//   - 长度前缀只读到一部分，或者长度前缀读完但 payload 没读完：
//     这是帧读到一半，io.ReadFull 在这两种情况下都返回 io.ErrUnexpectedEOF——
//     除了一种情况需要手动补上：长度前缀完整收到（也就是已经承诺了这一帧的
//     存在）之后，payload 一个字节都没读到就 EOF 了。io.ReadFull 对"目标切片
//     一个字节都没读到"统一报 io.EOF，这里必须改写成 io.ErrUnexpectedEOF，
//     否则这种"头收到了、body 一个字节都没来"的死亡会被误判成干净的流结束。
func ReadFrame(r io.Reader) ([]byte, error) {
	var hdr [4]byte
	if _, err := io.ReadFull(r, hdr[:]); err != nil {
		// 零字节时 io.ReadFull 返回 io.EOF（干净结束）；
		// 读到 1-3 字节后 EOF 时它已经返回 io.ErrUnexpectedEOF（帧读到一半）。
		// 两种情况都原样传给调用方，不需要改写。
		return nil, err
	}
	n := binary.BigEndian.Uint32(hdr[:])
	if n > MaxFrame {
		return nil, fmt.Errorf("control frame claims %d bytes, over the %d byte limit", n, MaxFrame)
	}
	b := make([]byte, n)
	if _, err := io.ReadFull(r, b); err != nil {
		if err == io.EOF {
			// 长度前缀已经完整收到,就已经承诺了这一帧的存在;
			// payload 却一个字节都没读到,是对端在帧中间死了,
			// 不能被上层当成流的干净结束来处理。
			return nil, io.ErrUnexpectedEOF
		}
		return nil, err
	}
	return b, nil
}
