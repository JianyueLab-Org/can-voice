package main

import (
	"encoding/json"
	"os"
	"strings"
	"testing"
)

func TestWriteReportProducesReadableJSON(t *testing.T) {
	dir := t.TempDir()
	r := Report{
		SchemaVersion: 1,
		ProbeVersion:  "p1-test",
		OS:            "darwin",
		Carrier:       "中国电信",
		Handshake:     HandshakeResult{OK: true, Millis: 42},
		Datagram:      RoundResult{Sent: 3000, Received: 2970, LossPercent: 1.0},
		Stream:        RoundResult{Sent: 3000, Received: 3000},
	}
	path, err := WriteReport(r, dir)
	if err != nil {
		t.Fatalf("WriteReport: %v", err)
	}
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read back: %v", err)
	}
	var got Report
	if err := json.Unmarshal(b, &got); err != nil {
		t.Fatalf("report is not valid JSON: %v", err)
	}
	if got.Carrier != "中国电信" {
		t.Fatalf("Carrier = %q, want 中国电信", got.Carrier)
	}
	if got.Datagram.Sent != 3000 {
		t.Fatalf("Datagram.Sent = %d, want 3000", got.Datagram.Sent)
	}
	if !strings.Contains(string(b), "\n") {
		t.Fatal("report must be indented; the user is asked to read it before sending it")
	}
}

// TestHandshakeFailureReportMarksBothRoundsFailedNotZero 钉住修复轮 1 的
// 修复 1：握手失败时，main.go 会调用 markRoundsAsNotRun 把两轮都标成
// Failed:true，而不是让它们停在零值——零值的 RoundResult 在 JSON 里跟
// "跑完了、一个没丢"长得一模一样，这正是本该防住的坑。
func TestHandshakeFailureReportMarksBothRoundsFailedNotZero(t *testing.T) {
	dir := t.TempDir()
	r := Report{
		SchemaVersion: 1,
		ProbeVersion:  "p1-test",
		OS:            "linux",
		Carrier:       "测试网络",
		Handshake:     HandshakeResult{OK: false, Millis: 5000, Error: "context deadline exceeded"},
	}
	markRoundsAsNotRun(&r)

	path, err := WriteReport(r, dir)
	if err != nil {
		t.Fatalf("WriteReport: %v", err)
	}
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read back: %v", err)
	}

	var got Report
	if err := json.Unmarshal(b, &got); err != nil {
		t.Fatalf("report is not valid JSON: %v", err)
	}
	if !got.Datagram.Failed {
		t.Fatalf("Datagram.Failed = false, want true when handshake failed; raw JSON:\n%s", b)
	}
	if !got.Stream.Failed {
		t.Fatalf("Stream.Failed = false, want true when handshake failed; raw JSON:\n%s", b)
	}
	// 反面断言：不能是"failed: false 配一串零"那个旧形状。
	if strings.Contains(string(b), `"failed": false`) {
		t.Fatalf("a round with failed:false next to a handshake failure reads as a completed, lossless round; raw JSON:\n%s", b)
	}
}

func TestSummariseReportNamesTheDecisiveComparison(t *testing.T) {
	r := Report{
		Handshake: HandshakeResult{OK: true, Millis: 42},
		Datagram:  RoundResult{Sent: 3000, Received: 0, Interrupted: true},
		Stream:    RoundResult{Sent: 3000, Received: 3000},
	}
	s := Summarise(r)
	if !strings.Contains(s, "datagram") && !strings.Contains(s, "数据报") {
		t.Fatalf("summary must name which channel failed, got:\n%s", s)
	}
	if !strings.Contains(s, "100") && !strings.Contains(s, "全部") {
		t.Fatalf("summary must make total datagram loss obvious, got:\n%s", s)
	}
}
