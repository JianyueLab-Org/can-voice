// Package geo 是 can-voice 的射程与信号质量计算。
//
// 纯数学，无 I/O。射程过滤放在服务端而不是客户端，是因为带宽：
// 全球 200 人同时在 121.500 时，客户端要收下 200 路流才能丢掉 199 路，
// 约 5.8 Mbps 下行（spec 7.1）。客户端衰减解决"吵"，解决不了"爆炸"。
package geo

import (
	"math"
	"strings"
)

// 信号质量曲线的两个拐点（spec 7.4）。
const (
	// FullRatio 以内是满格。
	FullRatio = 0.8
	// CutoffRatio 以外服务端不扇出。它比真实射程放宽 10%，
	// 中间这段边缘带交给客户端做平滑衰减——否则飞机停在射程线上时
	// 音频会忽有忽无。
	CutoffRatio = 1.1
)

const earthRadiusNM = 3440.065

// DistanceNM 是两点间的大圆距离，单位海里。
func DistanceNM(lat1, lon1, lat2, lon2 float64) float64 {
	φ1, φ2 := lat1*math.Pi/180, lat2*math.Pi/180
	dφ := (lat2 - lat1) * math.Pi / 180
	dλ := (lon2 - lon1) * math.Pi / 180
	a := math.Sin(dφ/2)*math.Sin(dφ/2) +
		math.Cos(φ1)*math.Cos(φ2)*math.Sin(dλ/2)*math.Sin(dλ/2)
	return 2 * earthRadiusNM * math.Asin(math.Min(1, math.Sqrt(a)))
}

// LineOfSightNM 是 VHF 视距射程：1.23 × (√h₁ + √h₂)，高度单位英尺。
//
// 这个公式本身就产生了正确的行为——地面上听不到远处、高空能听很远——
// 所以飞行员之间不需要任何额外规则。管制席位不能用它：一个 ACC 席位
// 现实中是一组分布式电台，见 FallbackRangeNM 与 datafeed 的 visual_range。
func LineOfSightNM(alt1Ft, alt2Ft float64) float64 {
	return 1.23 * (math.Sqrt(math.Max(0, alt1Ft)) + math.Sqrt(math.Max(0, alt2Ft)))
}

// suffixRange 是席位后缀的兜底半径，单位海里。
//
// 只在 can-fsd datafeed 的 visual_range 为 0 时使用——那是权威值，
// 由管制员在 #AA 里声明。ATIS 席位的 visual_range 就常常是 0。
// 这张表不是权威来源，只是权威字段缺失时的兜底。
//
// 这张表的数值是估的，需要按中国 FIR 的实际尺寸校准（spec 12）。
// 它是服务端配置而非编译期常量的理由也在这里：调它不该需要发版。
var suffixRange = map[string]float64{
	"DEL":  15,
	"GND":  15,
	"TWR":  30,
	"APP":  80,
	"DEP":  80,
	"CTR":  250,
	"FSS":  600,
	"ATIS": 60,
}

// unknownSuffixRange 是认不出后缀时的保守默认值。
// 刻意不是 0：0 会让那个席位谁都听不见，而在语音系统里
// "听不见"比"听得太远"糟糕得多。
const unknownSuffixRange = 80

// FallbackRangeNM 按呼号后缀给出兜底半径。
//
// 这只是兜底：真正的权威来源是 can-fsd datafeed 里管制员自己声明的
// visual_range 字段。只有当那个字段是 0（未声明）时才落到这张后缀表上。
func FallbackRangeNM(callsign string) float64 {
	i := strings.LastIndex(callsign, "_")
	if i < 0 {
		return unknownSuffixRange
	}
	if r, ok := suffixRange[strings.ToUpper(callsign[i+1:])]; ok {
		return r
	}
	return unknownSuffixRange
}

// Quality 把距离与射程之比折算成 0–255 的信号质量。
// inRange 为 false 时服务端不扇出这一包。
//
// 客户端只会收到 qual 大于 0 的包，qual 越低说明越接近射程边缘——
// 它因此完全不知道别人在哪，隐私与防作弊是免费拿到的（spec 7.4）。
func Quality(distNM, rangeNM float64) (uint8, bool) {
	// range 为 0 意味着上游没给出有效射程。宁可不扇出，也不要把它当成无限远。
	if rangeNM <= 0 {
		return 0, false
	}
	ratio := distNM / rangeNM
	switch {
	case ratio <= FullRatio:
		return 255, true
	case ratio >= CutoffRatio:
		return 0, false
	default:
		frac := (CutoffRatio - ratio) / (CutoffRatio - FullRatio)
		q := int(math.Round(frac * 255))
		if q < 1 {
			// 还在射程内就不能报 0——0 是"出界"的信号。
			q = 1
		}
		return uint8(q), true
	}
}
