package control

import (
	"bytes"
	"errors"
	"io"
	"strings"
	"testing"
)

func TestFrameRoundTrip(t *testing.T) {
	var buf bytes.Buffer
	payload := []byte(`{"type":"PING","t":1}`)
	if err := WriteFrame(&buf, payload); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	got, err := ReadFrame(&buf)
	if err != nil {
		t.Fatalf("ReadFrame: %v", err)
	}
	if string(got) != string(payload) {
		t.Fatalf("frame = %q, want %q", got, payload)
	}
}

func TestWriteFrameRejectsOversizedPayload(t *testing.T) {
	var buf bytes.Buffer
	if err := WriteFrame(&buf, make([]byte, MaxFrame+1)); err == nil {
		t.Fatal("WriteFrame must reject a payload over MaxFrame")
	}
}

func TestReadFrameRejectsOversizedLengthPrefix(t *testing.T) {
	// 一个恶意的长度前缀不能让服务端去分配 4 GB。
	hdr := []byte{0xff, 0xff, 0xff, 0xff}
	if _, err := ReadFrame(bytes.NewReader(hdr)); err == nil {
		t.Fatal("ReadFrame must reject a length prefix over MaxFrame before allocating")
	} else if !strings.Contains(err.Error(), "limit") {
		t.Fatalf("error should name the limit, got %v", err)
	}
}

// 上一帧读完、下一帧还没开始时对端关闭：这是正常的流结束，
// 调用方（后续任务的连接循环）不应该把它当成错误来报警。
func TestReadFrameCleanEndOfStreamIsEOF(t *testing.T) {
	_, err := ReadFrame(bytes.NewReader(nil))
	if !errors.Is(err, io.EOF) {
		t.Fatalf("ReadFrame at a frame boundary = %v, want io.EOF", err)
	}
}

// 长度前缀只送到一半就断了：这是帧读到一半，必须能和干净的流结束区分开。
func TestReadFrameTruncatedLengthPrefixIsNotCleanEOF(t *testing.T) {
	_, err := ReadFrame(bytes.NewReader([]byte{0x00, 0x00}))
	if err == nil {
		t.Fatal("ReadFrame must reject a truncated length prefix")
	}
	if errors.Is(err, io.EOF) {
		t.Fatalf("a truncated length prefix must not read as a clean io.EOF, got %v", err)
	}
	if !errors.Is(err, io.ErrUnexpectedEOF) {
		t.Fatalf("a truncated length prefix should be io.ErrUnexpectedEOF, got %v", err)
	}
}

// 长度前缀完整收到（承诺了一帧的存在），但 payload 一个字节都没送到就断了：
// 这也是帧读到一半，而不是流的干净结束——即便 io.ReadFull 对"目标切片
// 一个字节都没读到"这一种情况天然返回的是 io.EOF，这里也必须改写。
func TestReadFrameTruncatedPayloadIsNotCleanEOF(t *testing.T) {
	var hdr [4]byte
	hdr[3] = 5 // 声明 5 字节 payload，但一个字节都不给
	_, err := ReadFrame(bytes.NewReader(hdr[:]))
	if err == nil {
		t.Fatal("ReadFrame must reject a header promising a payload that never arrives")
	}
	if errors.Is(err, io.EOF) {
		t.Fatalf("a promised-but-missing payload must not read as a clean io.EOF, got %v", err)
	}
	if !errors.Is(err, io.ErrUnexpectedEOF) {
		t.Fatalf("a promised-but-missing payload should be io.ErrUnexpectedEOF, got %v", err)
	}
}

// payload 送了一部分就断了：同样是帧读到一半。
func TestReadFrameTruncatedPartialPayloadIsNotCleanEOF(t *testing.T) {
	var hdr [4]byte
	hdr[3] = 5 // 声明 5 字节 payload
	buf := append(hdr[:], []byte{1, 2}...)
	_, err := ReadFrame(bytes.NewReader(buf))
	if errors.Is(err, io.EOF) {
		t.Fatalf("a partially delivered payload must not read as a clean io.EOF, got %v", err)
	}
	if !errors.Is(err, io.ErrUnexpectedEOF) {
		t.Fatalf("a partially delivered payload should be io.ErrUnexpectedEOF, got %v", err)
	}
}

// 一个声明长度为 0 的帧是合法的空帧，不是错误。
func TestReadFrameZeroLengthPayloadIsNotAnError(t *testing.T) {
	var hdr [4]byte // 全 0：长度前缀声明 0 字节 payload
	got, err := ReadFrame(bytes.NewReader(hdr[:]))
	if err != nil {
		t.Fatalf("ReadFrame of a zero-length frame: %v", err)
	}
	if len(got) != 0 {
		t.Fatalf("frame = %q, want empty", got)
	}
}
