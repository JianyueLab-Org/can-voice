package main

import (
	"encoding/binary"
	"fmt"
	"io"
)

// maxFrame 比 QUIC datagram 的上限宽裕，够放下测量载荷即可。
const maxFrame = 1400

// writeFrame 写一个 2 字节大端长度前缀加载荷。
func writeFrame(w io.Writer, b []byte) error {
	if len(b) > maxFrame {
		return fmt.Errorf("frame of %d bytes exceeds the %d byte limit", len(b), maxFrame)
	}
	var hdr [2]byte
	binary.BigEndian.PutUint16(hdr[:], uint16(len(b)))
	if _, err := w.Write(hdr[:]); err != nil {
		return err
	}
	_, err := w.Write(b)
	return err
}

// readFrame 读一个长度前缀帧。
func readFrame(r io.Reader) ([]byte, error) {
	var hdr [2]byte
	if _, err := io.ReadFull(r, hdr[:]); err != nil {
		return nil, err
	}
	n := binary.BigEndian.Uint16(hdr[:])
	if n > maxFrame {
		return nil, fmt.Errorf("frame claims %d bytes, over the %d byte limit", n, maxFrame)
	}
	b := make([]byte, n)
	if _, err := io.ReadFull(r, b); err != nil {
		return nil, err
	}
	return b, nil
}
