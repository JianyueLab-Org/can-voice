package fsdfeed

import (
	"math"
	"os"
	"testing"
)

func sample(t *testing.T) Snapshot {
	t.Helper()
	b, err := os.ReadFile("../../testdata/datafeed_sample.json")
	if err != nil {
		t.Fatalf("read sample: %v", err)
	}
	s, err := ParseDatafeed(b)
	if err != nil {
		t.Fatalf("ParseDatafeed: %v", err)
	}
	return s
}

// 这是本包最重要的测试。can-fsd 的 datafeed 里飞行员的经纬度是 JSON 数字，
// 管制员和 ATIS 的是 JSON 字符串。只当数字解析的话所有管制员会落在 0,0
// （几内亚湾），表现为管制员谁都听不见，而日志里一条错误都没有。
func TestControllerCoordinatesParseFromStrings(t *testing.T) {
	s := sample(t)
	p, ok := s.ByCallsign["ZSHA_CTR"]
	if !ok {
		t.Fatal("ZSHA_CTR missing from the snapshot")
	}
	if math.Abs(p.Lat-31.20466) > 0.001 || math.Abs(p.Lon-121.45272) > 0.001 {
		t.Fatalf("ZSHA_CTR at %v,%v — controller coordinates are JSON strings and must still parse", p.Lat, p.Lon)
	}
	if p.Lat == 0 && p.Lon == 0 {
		t.Fatal("ZSHA_CTR landed at 0,0: the string/number asymmetry was not handled")
	}
}

func TestPilotCoordinatesParseFromNumbers(t *testing.T) {
	s := sample(t)
	p, ok := s.ByCallsign["CCA5852"]
	if !ok {
		t.Fatal("CCA5852 missing from the snapshot")
	}
	if math.Abs(p.Lat-25.10232) > 0.001 || math.Abs(p.Lon-102.933) > 0.001 {
		t.Fatalf("CCA5852 at %v,%v, want 25.10232,102.933", p.Lat, p.Lon)
	}
	if p.AltFt != 6897 {
		t.Fatalf("CCA5852 altitude = %v, want 6897", p.AltFt)
	}
}

func TestPilotRangeComesFromAltitudeNotVisualRange(t *testing.T) {
	// 飞行员的 visual_range 在样本里是 40，但 VHF 视距由高度决定：
	// 6897 英尺约 102 海里。用 visual_range 会把射程砍掉 60%。
	s := sample(t)
	p := s.ByCallsign["CCA5852"]
	if p.RangeNM < 90 || p.RangeNM > 115 {
		t.Fatalf("pilot RangeNM = %.0f, want about 102 (line of sight from 6897 ft)", p.RangeNM)
	}
}

func TestControllerRangeUsesVisualRange(t *testing.T) {
	s := sample(t)
	if p := s.ByCallsign["ZSHA_CTR"]; p.RangeNM != 600 {
		t.Fatalf("ZSHA_CTR RangeNM = %v, want 600 from visual_range", p.RangeNM)
	}
}

func TestZeroVisualRangeFallsBackToTheSuffixTable(t *testing.T) {
	// 样本里 ZSSS_ATIS 的 visual_range 正是 0。
	s := sample(t)
	p, ok := s.ByCallsign["ZSSS_ATIS"]
	if !ok {
		t.Fatal("ZSSS_ATIS missing from the snapshot")
	}
	if p.RangeNM != 60 {
		t.Fatalf("ZSSS_ATIS RangeNM = %v, want the 60 nm _ATIS fallback (its visual_range is 0)", p.RangeNM)
	}
}

func TestSnapshotIsIndexedByCID(t *testing.T) {
	// cid 是关联键：token 里本来就有它，所以正常情况下协议里一个字段都不用加。
	s := sample(t)
	if _, ok := s.ByCID["1012"]; !ok {
		t.Fatal("snapshot must be indexed by cid; 1012 (CCA5852) is missing")
	}
}

func TestParseDatafeedRejectsGarbage(t *testing.T) {
	if _, err := ParseDatafeed([]byte("not json")); err == nil {
		t.Fatal("ParseDatafeed must reject non-JSON input")
	}
}
