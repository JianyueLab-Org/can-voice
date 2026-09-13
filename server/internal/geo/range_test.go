package geo

import (
	"math"
	"testing"
)

func TestLineOfSightMatchesTheSpecExamples(t *testing.T) {
	// spec 7.2 的三个例子。
	cases := []struct {
		name       string
		a, b       float64
		wantAround float64
	}{
		{"FL350 对 100 英尺地面台", 35000, 100, 240},
		{"两架 FL350 之间", 35000, 35000, 460},
		{"地面两架之间", 0, 0, 0},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			got := LineOfSightNM(c.a, c.b)
			if math.Abs(got-c.wantAround) > 15 {
				t.Fatalf("LineOfSightNM(%v, %v) = %.0f, want about %.0f", c.a, c.b, got, c.wantAround)
			}
		})
	}
}

func TestFallbackRangeReadsTheSuffix(t *testing.T) {
	cases := map[string]float64{
		"ZSSS_GND":  15,
		"ZSSS_DEL":  15,
		"ZSPD_TWR":  30,
		"ZSSS_APP":  80,
		"ZGGG_DEP":  80,
		"ZSHA_CTR":  250,
		"PRC_FSS":   600,
		"ZSSS_ATIS": 60,
	}
	for cs, want := range cases {
		if got := FallbackRangeNM(cs); got != want {
			t.Fatalf("FallbackRangeNM(%q) = %v, want %v", cs, got, want)
		}
	}
}

func TestFallbackRangeHandlesAnUnknownSuffix(t *testing.T) {
	// 未知后缀必须给一个保守的非零值：给 0 会让那个席位谁都听不见，
	// 而这在语音系统里比听得太远糟糕得多。
	if got := FallbackRangeNM("ZSSS_XYZ"); got <= 0 {
		t.Fatalf("FallbackRangeNM on an unknown suffix = %v, must be a conservative non-zero default", got)
	}
}

func TestQualityIsFullInsideEightyPercent(t *testing.T) {
	for _, ratio := range []float64{0, 0.3, 0.79, 0.8} {
		q, in := Quality(ratio*100, 100)
		if !in || q != 255 {
			t.Fatalf("Quality at d/range=%.2f = (%d, %v), want (255, true)", ratio, q, in)
		}
	}
}

func TestQualityFallsLinearlyInTheEdgeBand(t *testing.T) {
	// 0.8 → 1.1 之间线性 255 → 0。中点 0.95 应当接近 127。
	q, in := Quality(95, 100)
	if !in {
		t.Fatal("d/range=0.95 must still be in range")
	}
	if q < 120 || q > 135 {
		t.Fatalf("Quality at d/range=0.95 = %d, want about 127", q)
	}
}

func TestQualityIsOutOfRangeBeyondTheCutoff(t *testing.T) {
	for _, ratio := range []float64{1.1, 1.11, 5} {
		if _, in := Quality(ratio*100, 100); in {
			t.Fatalf("d/range=%.2f must be out of range", ratio)
		}
	}
}

func TestQualityTreatsAZeroRangeAsOutOfRange(t *testing.T) {
	// range 为 0 意味着上游没给出有效射程。宁可不扇出，也不要除零。
	if _, in := Quality(0, 0); in {
		t.Fatal("a zero range must be treated as out of range, not as infinite range")
	}
}

func TestDistanceBetweenKnownAirports(t *testing.T) {
	// ZSSS (31.198, 121.336) 到 ZBAA (40.080, 116.585) 约 581 海里。
	got := DistanceNM(31.198, 121.336, 40.080, 116.585)
	if math.Abs(got-581) > 5 {
		t.Fatalf("DistanceNM ZSSS→ZBAA = %.0f, want about 581", got)
	}
}

func TestDistanceIsZeroForTheSamePoint(t *testing.T) {
	if got := DistanceNM(31.198, 121.336, 31.198, 121.336); got > 0.001 {
		t.Fatalf("DistanceNM to the same point = %v, want 0", got)
	}
}
