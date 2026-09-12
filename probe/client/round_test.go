package main

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/quic-go/quic-go"
)

func TestProbeRoundTripsThroughEncoding(t *testing.T) {
	now := time.Unix(1757000000, 123456789)
	b := encodeProbe(42, now, 73)
	if len(b) != 73 {
		t.Fatalf("encodeProbe produced %d bytes, want 73 (must match the real voice payload)", len(b))
	}
	p, err := decodeProbe(b)
	if err != nil {
		t.Fatalf("decodeProbe: %v", err)
	}
	if p.Seq != 42 {
		t.Fatalf("Seq = %d, want 42", p.Seq)
	}
	if p.SentUnixNanos != now.UnixNano() {
		t.Fatalf("SentUnixNanos = %d, want %d", p.SentUnixNanos, now.UnixNano())
	}
}

func TestDecodeProbeRejectsShortPayload(t *testing.T) {
	if _, err := decodeProbe([]byte{1, 2, 3}); err == nil {
		t.Fatal("decodeProbe must reject a payload shorter than the 12 byte header")
	}
}

func TestSummariseMarksAnInterruptedRound(t *testing.T) {
	// 60 秒轮次、50 pps：前 55 秒全收到，最后 5 秒全丢。
	const rate = 50
	rtts := make(map[uint32]time.Duration)
	for seq := uint32(0); seq < 55*rate; seq++ {
		rtts[seq] = 30 * time.Millisecond
	}
	got := summarise(60*rate, rtts, rate)
	if !got.Interrupted {
		t.Fatal("a round whose last 5 seconds received nothing must be marked Interrupted")
	}
	if got.Received != 55*rate {
		t.Fatalf("Received = %d, want %d", got.Received, 55*rate)
	}
}

func TestSummariseDoesNotMarkUniformLossAsInterrupted(t *testing.T) {
	// 均匀丢 10%，一直到最后都有回显 —— 这需要抖动缓冲，不需要 stream 回退。
	const rate = 50
	rtts := make(map[uint32]time.Duration)
	for seq := uint32(0); seq < 60*rate; seq++ {
		if seq%10 == 0 {
			continue
		}
		rtts[seq] = 30 * time.Millisecond
	}
	got := summarise(60*rate, rtts, rate)
	if got.Interrupted {
		t.Fatal("uniform loss must not be reported as an interruption")
	}
	if got.LossPercent < 9 || got.LossPercent > 11 {
		t.Fatalf("LossPercent = %.1f, want about 10", got.LossPercent)
	}
}

func TestStreamRoundUsesTheSameSummaryShape(t *testing.T) {
	// summarise 对两轮是共用的（三个参数：sent、rtts、rate，没有 d）；
	// 这个测试锁住"stream 轮不自己发明一套统计"。
	const rate = 50
	rtts := map[uint32]time.Duration{0: 10 * time.Millisecond}
	got := summarise(1, rtts, rate)
	if got.Sent != 1 || got.Received != 1 || got.LossPercent != 0 {
		t.Fatalf("summarise = %+v, want sent=1 received=1 loss=0", got)
	}
}

// fakeConnFailsOpenStreamSync 只覆盖 RunStreamRound 在失败路径上会调用的那一个
// 方法；其余方法通过内嵌一个 nil 的 quic.Connection 来满足接口签名——这条测试
// 从不会走到它们，调用了才会 panic。
type fakeConnFailsOpenStreamSync struct {
	quic.Connection
	err error
}

func (f fakeConnFailsOpenStreamSync) OpenStreamSync(ctx context.Context) (quic.Stream, error) {
	return nil, f.err
}

func TestStreamRoundReportsFailureInsteadOfZeros(t *testing.T) {
	// Ruling D：一轮连 stream 都开不出来，绝不能报成 Sent=0 Received=0
	// LossPercent=0——那和"跑完了、一个没丢"在 JSON 里长得一模一样，
	// 会把"两轮差异"这个结论读反。错误文本里带一个假的对端地址，
	// 顺带验证它在进入 Error 字段之前被 scrubAddrs 清洗过。
	fake := fakeConnFailsOpenStreamSync{
		err: errors.New("dial udp 192.168.1.23:54321->203.0.113.7:4433: connect: no route to host"),
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()

	got := RunStreamRound(ctx, fake, time.Second)
	if !got.Failed {
		t.Fatal("Failed must be true when OpenStreamSync fails")
	}
	if got.Sent != 0 || got.Received != 0 || got.LossPercent != 0 || got.Interrupted {
		t.Fatalf("a failed round must not carry measurement-shaped zeros as if they were real: %+v", got)
	}
	if got.Error == "" {
		t.Fatal("Error must carry the failure reason")
	}
	if strings.Contains(got.Error, "192.168.1.23") || strings.Contains(got.Error, "203.0.113.7") {
		t.Fatalf("Error must be scrubbed of IP addresses, got %q", got.Error)
	}
}
