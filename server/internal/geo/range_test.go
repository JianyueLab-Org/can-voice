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

// TestQualityNeverReportsZeroWhileStillInRange 钉住衰减带最外侧那道夹紧。
//
// 比值落在 (1.09941, 1.1) 这一段时，`frac × 255` 四舍五入就是 0——而
// `(0, true)` 是一个自相矛盾的答案："这一包要投递，信号质量为零"。
// 下游把它当真的话，wire 包那条写给接收端的承诺（下行 qual 恒在 1–255，
// 0 不可能来自服务端）当场作废，而接收端那条"最弱信号"分支本来就不该存在。
//
// 这一段此前**一个用例都没有**：0.95 在中间，1.1 和 1.11 在外面，
// 整个夹紧分支删掉整套测试照绿。上界取 1.09999 而不是无限贴近 1.1，
// 是因为再往上浮点就分辨不出来了；下界那一档（raw 恰好在 0.5 附近）
// 交给上面的线性测试。
func TestQualityNeverReportsZeroWhileStillInRange(t *testing.T) {
	// 1.09941 是 `round(frac×255)` 由 1 掉到 0 的那个点：
	// frac = (1.1-ratio)/0.3，255×frac < 0.5 ⟺ ratio > 1.1 - 0.3/510 = 1.0994117…
	for _, ratio := range []float64{1.09942, 1.0997, 1.09999} {
		q, in := Quality(ratio*100, 100)
		if !in {
			t.Fatalf("d/range=%.5f must still be in range — the cutoff is %.2f", ratio, CutoffRatio)
		}
		// 前提：这组输入真的落在"不夹就会是 0"的那一段里。少了这一句，
		// 把 ratio 写成 1.05 也能让下面那句绿，而那时夹紧删掉照样绿。
		if raw := (CutoffRatio - ratio) / (CutoffRatio - FullRatio) * 255; raw >= 0.5 {
			t.Fatalf("premise: d/range=%.5f gives an unclamped quality of %.4f, which already rounds to 1 or more — this input cannot tell the clamp from its absence", ratio, raw)
		}
		if q != 1 {
			t.Fatalf("Quality at d/range=%.5f = %d, want 1 — inside the cutoff the answer may not be 0: (0, true) says \"deliver this packet, signal strength zero\", and it breaks the wire-level promise that a downlink qual is never 0", ratio, q)
		}
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

// TestLOSTermIsHalfOfTheLineOfSight 钉住两个函数的关系。
// 射程公式是 1.23×(√h₁+√h₂)，每个参与者贡献自己的那一项；分开之后
// 必须还是原来那个和，否则飞行员之间的射程就变了。
func TestLOSTermIsHalfOfTheLineOfSight(t *testing.T) {
	for _, c := range []struct{ a, b float64 }{{0, 0}, {35000, 35000}, {1000, 41000}, {6897, 0}} {
		want := LineOfSightNM(c.a, c.b)
		got := LOSTermNM(c.a) + LOSTermNM(c.b)
		if math.Abs(got-want) > 1e-9 {
			t.Fatalf("LOSTermNM(%v)+LOSTermNM(%v) = %v, LineOfSightNM = %v", c.a, c.b, got, want)
		}
	}
}

func TestLOSTermClampsNegativeAltitude(t *testing.T) {
	// 死海边上的机场是负的 MSL 高度，NaN 会一路传染到信号质量里。
	if got := LOSTermNM(-1300); got != 0 {
		t.Fatalf("LOSTermNM(-1300) = %v, want 0", got)
	}
}

func TestFallbackRangeIgnoresSurroundingWhitespace(t *testing.T) {
	// 呼号来自网络。带空白的后缀认不出来就掉到 80 海里的默认值上，
	// 一个 GND 席位（15 海里）会变成 80，听到五倍远的地方。
	if got, want := FallbackRangeNM(" ZSPD_TWR "), 30.0; got != want {
		t.Fatalf("FallbackRangeNM(%q) = %v, want %v", " ZSPD_TWR ", got, want)
	}
}

func TestFallbackRangeTableCases(t *testing.T) {
	for _, c := range []struct {
		callsign string
		want     float64
	}{
		{"ZSPD_GND", 15},
		{"ZSPD_TWR", 30},
		{"ZSSS_1_TWR", 30}, // 多个下划线：取最后一段
		{"ZSHA_CTR", 250},
		{"PRC_FSS", 600},
		{"ZSSS_ATIS", 60},
		{"ZSPD_XYZ", 80},     // 认不出的后缀
		{"NOUNDERSCORE", 80}, // 根本没有下划线
		{"zspd_twr", 30},     // 大小写不敏感
		{"", 80},
	} {
		if got := FallbackRangeNM(c.callsign); got != c.want {
			t.Errorf("FallbackRangeNM(%q) = %v, want %v", c.callsign, got, c.want)
		}
	}
}

func TestDistanceEdgeCases(t *testing.T) {
	// 对跖点：地球半周长。
	if got, want := DistanceNM(0, 0, 0, 180), math.Pi*3440.065; math.Abs(got-want) > 0.5 {
		t.Fatalf("antipodal distance = %v, want about %v", got, want)
	}
	// 跨 180° 经线：179°E 到 179°W 只有 2 个经度，不是 358 个。
	got := DistanceNM(0, 179, 0, -179)
	if got > 130 {
		t.Fatalf("distance across the antimeridian = %v NM, want about 120 — the formula went the long way round", got)
	}
	// 同一点。
	if got := DistanceNM(31.2, 121.3, 31.2, 121.3); got != 0 {
		t.Fatalf("distance to self = %v, want 0", got)
	}
}
