package main

import (
	"testing"
	"time"
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
