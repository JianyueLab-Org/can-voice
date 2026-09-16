// Package geo 是 can-voice 的射程与信号质量计算。
//
// 纯数学，无 I/O。射程过滤放在服务端而不是客户端，是因为带宽：
// 全球 200 人同时在 121.500 时，客户端要收下 200 路流才能丢掉 199 路，
// 约 5.8 Mbps 下行（spec 7.1）。客户端衰减解决"吵"，解决不了"爆炸"。
package geo

import (
	"fmt"
	"math"
	"strconv"
	"strings"
	"sync/atomic"
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

// LOSTermNM 是视距公式里属于一个参与者的那一项：1.23√h，高度单位英尺。
//
// 拆出来是因为两个参与者的射程是两项之**和**，而这个和必须在知道双方高度时
// 才算得出来——把 1.23√h 存进某个人的"射程"字段，再在两个人之间取 max，
// 会把两架 FL350 的 460 海里变成 230。
func LOSTermNM(altFt float64) float64 {
	return 1.23 * math.Sqrt(math.Max(0, altFt))
}

// LineOfSightNM 是 VHF 视距射程：1.23 × (√h₁ + √h₂)，高度单位英尺。
//
// 这个公式本身就产生了正确的行为——地面上听不到远处、高空能听很远——
// 所以飞行员之间不需要任何额外规则。管制席位不能用它：一个 ACC 席位
// 现实中是一组分布式电台，见 FallbackRangeNM 与 datafeed 的 visual_range。
func LineOfSightNM(alt1Ft, alt2Ft float64) float64 {
	return LOSTermNM(alt1Ft) + LOSTermNM(alt2Ft)
}

// builtinSuffixRange 是席位后缀的兜底半径，单位海里。
//
// 只在 can-fsd datafeed 的 visual_range 为 0 时使用——那是权威值，
// 由管制员在 #AA 里声明。ATIS 席位的 visual_range 就常常是 0。
// 这张表不是权威来源，只是权威字段缺失时的兜底。
//
// 这张表的数值是估的，需要按中国 FIR 的实际尺寸校准（spec 12）。
// 它因此是**内置默认值而不是最终答案**：`CAN_VOICE_SUFFIX_RANGES` 可以逐条
// 覆盖它，校准一次半径不该需要改代码、重新发版。
var builtinSuffixRange = map[string]float64{
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

// unknownKey 是覆盖串里代表"认不出后缀"那一档的键。
//
// 和表里的条目共用一个环境变量，是因为它们是同一件事的两半：
// 分成两个变量只会让人调了一个忘了另一个。
const unknownKey = "*"

// Table 是一份兜底半径表。
type Table struct {
	bySuffix map[string]float64
	unknown  float64
}

// DefaultTable 返回内置的那一份。
func DefaultTable() *Table {
	m := make(map[string]float64, len(builtinSuffixRange))
	for k, v := range builtinSuffixRange {
		m[k] = v
	}
	return &Table{bySuffix: m, unknown: unknownSuffixRange}
}

// ParseTable 读一份覆盖串：`CTR=300,FSS=700,*=120`，单位海里。
//
// **覆盖是逐条的**，没有提到的后缀照用内置值：只想把 CTR 调大的人不必把整张表
// 重打一遍，而重打一遍的那份拷贝一旦漏了一行，漏掉的那个席位会悄悄掉到"认不出
// 后缀"的默认值上。
//
// 写坏了返回错误而不是悄悄回退——悄悄回退的话，一个打错了一个字符的运维以为
// 自己校准过了，而服务端跑的还是估出来的那张表，没有任何地方会告诉他。
func ParseTable(spec string) (*Table, error) {
	t := DefaultTable()
	spec = strings.TrimSpace(spec)
	if spec == "" {
		return t, nil
	}
	for _, entry := range strings.Split(spec, ",") {
		entry = strings.TrimSpace(entry)
		key, value, ok := strings.Cut(entry, "=")
		if !ok {
			return nil, fmt.Errorf("suffix range %q is not in the form SUFFIX=NM", entry)
		}
		key = strings.ToUpper(strings.TrimSpace(key))
		if key == "" {
			return nil, fmt.Errorf("suffix range %q has an empty suffix", entry)
		}
		nm, err := strconv.ParseFloat(strings.TrimSpace(value), 64)
		if err != nil {
			return nil, fmt.Errorf("suffix range %q is not a number: %w", entry, err)
		}
		// 0 会让那个席位谁都听不见。想让一个席位安静下来的办法不是把射程配成 0。
		if nm <= 0 {
			return nil, fmt.Errorf("suffix range %q must be greater than zero", entry)
		}
		if key == unknownKey {
			t.unknown = nm
			continue
		}
		t.bySuffix[key] = nm
	}
	return t, nil
}

// RangeNM 按呼号后缀给出兜底半径。
func (t *Table) RangeNM(callsign string) float64 {
	callsign = strings.TrimSpace(callsign)
	i := strings.LastIndex(callsign, "_")
	if i < 0 {
		return t.unknown
	}
	if r, ok := t.bySuffix[strings.ToUpper(callsign[i+1:])]; ok {
		return r
	}
	return t.unknown
}

// current 是此刻生效的那张表。
//
// 用 atomic.Pointer 而不是普通的包级变量：它在启动时被配置覆盖一次，之后被
// 每一包的扇出路径读。普通变量的那次写和后面那些读之间没有 happens-before，
// `-race` 会在第一个并发用例上就把它抓出来。
var current atomic.Pointer[Table]

// UseTable 换掉此刻生效的表。**只在启动时调用。**
func UseTable(t *Table) {
	current.Store(t)
}

// FallbackRangeNM 按呼号后缀给出兜底半径。
//
// 这只是兜底：真正的权威来源是 can-fsd datafeed 里管制员自己声明的
// visual_range 字段。只有当那个字段是 0（未声明）时才落到这张后缀表上。
func FallbackRangeNM(callsign string) float64 {
	t := current.Load()
	if t == nil {
		// 没配过就是内置那份。**不是恐慌也不是 0**：一个还没走到 LoadConfig
		// 的调用方（测试、工具）拿到的应该是一个能用的答案。
		t = DefaultTable()
	}
	return t.RangeNM(callsign)
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
