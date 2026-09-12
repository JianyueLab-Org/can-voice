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
