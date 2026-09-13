package main

import (
	"strings"
	"testing"
)

func reportWith(carrier string, hsOK bool, dgLoss, stLoss float64, interrupted bool) Report {
	return Report{
		SchemaVersion: 1,
		Carrier:       carrier,
		Handshake:     HandshakeResult{OK: hsOK, Millis: 40},
		Datagram:      RoundResult{Sent: 3000, Received: 3000, LossPercent: dgLoss, Interrupted: interrupted},
		Stream:        RoundResult{Sent: 3000, Received: 3000, LossPercent: stLoss},
	}
}

func TestVerdictRefusesToDecideOnTooFewSessions(t *testing.T) {
	var rs []Report
	for i := 0; i < 7; i++ {
		rs = append(rs, reportWith("中国电信", true, 0, 0, false))
	}
	_, conclusive, reasons := Verdict(rs)
	if conclusive {
		t.Fatalf("Verdict must refuse below 8 sessions, got conclusive=true reasons=%v", reasons)
	}
	if len(reasons) == 0 || reasons[0][:6] != "sample" {
		t.Fatalf("Verdict must refuse below 8 sessions, got reasons=%v", reasons)
	}
}

func TestVerdictRefusesToDecideOnTooFewCarriers(t *testing.T) {
	var rs []Report
	for i := 0; i < 10; i++ {
		rs = append(rs, reportWith("中国电信", true, 0, 0, false))
	}
	_, conclusive, reasons := Verdict(rs)
	if conclusive {
		t.Fatalf("Verdict must refuse with fewer than 3 carriers, got conclusive=true reasons=%v", reasons)
	}
	if len(reasons) == 0 {
		t.Fatal("Verdict must refuse with fewer than 3 carriers")
	}
}

func TestVerdictSaysNoFallbackWhenEverythingIsClean(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通", "校园网"}
	var rs []Report
	for i := 0; i < 12; i++ {
		rs = append(rs, reportWith(carriers[i%len(carriers)], true, 0.3, 0.2, false))
	}
	need, conclusive, reasons := Verdict(rs)
	if need {
		t.Fatalf("clean data must not demand a fallback; reasons=%v", reasons)
	}
	if !conclusive {
		t.Fatalf("a fully adequate, clean sample must be conclusive; reasons=%v", reasons)
	}
	if len(reasons) != 0 {
		t.Fatalf("a clean sample with no exclusions must produce no reasons at all, got %v", reasons)
	}
}

// TestVerdictDemandsFallbackOnHandshakeFailureRate 单独钉住判定表第一行：
// 握手失败的会话比例 >= 2%。20 个会话里 1 个握手失败 = 5%，其余全部干净，
// 不触碰其余三行的阈值，确认只有第一行单独触发。
func TestVerdictDemandsFallbackOnHandshakeFailureRate(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通"}
	var rs []Report
	rs = append(rs, reportWith(carriers[0], false, 0, 0, false))
	for i := 0; i < 19; i++ {
		rs = append(rs, reportWith(carriers[i%3], true, 0.3, 0.2, false))
	}
	need, conclusive, reasons := Verdict(rs)
	if !need {
		t.Fatalf("1 of 20 sessions failing the handshake (5%%) is over the 2%% threshold; reasons=%v", reasons)
	}
	if !conclusive {
		t.Fatalf("this sample is adequate throughout; Verdict must be conclusive; reasons=%v", reasons)
	}
	if len(reasons) != 1 || !strings.Contains(reasons[0], "handshake failed") {
		t.Fatalf("expected exactly one handshake-failure reason, got %v", reasons)
	}
}

// TestVerdictInterruptRateUsesSessionsThatPassedHandshakeAsDenominator 钉住
// Ruling 3：interrupted 只在过了握手的会话里计数，分母必须跟着用"过了握手
// 的会话数"，而不是全部会话数——否则网络越差、握手失败越多，比例反而被
// 摊薄。25 个会话，5 个握手失败，20 个过了握手，其中 1 个 datagram 轮被
// 中断：1/20 = 5.0%，刚好压线触发；如果错误地除以 25，会得到 4.0%，
// 低于阈值、不会触发。
func TestVerdictInterruptRateUsesSessionsThatPassedHandshakeAsDenominator(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通"}
	var rs []Report
	for i := 0; i < 5; i++ {
		rs = append(rs, reportWith(carriers[i%3], false, 0, 0, false)) // 握手失败
	}
	for i := 0; i < 20; i++ {
		interrupted := i == 0 // 20 个过了握手的会话里恰好 1 个
		rs = append(rs, reportWith(carriers[i%3], true, 0.3, 0.2, interrupted))
	}
	_, _, reasons := Verdict(rs)
	found := false
	for _, r := range reasons {
		if strings.Contains(r, "sessions that passed the handshake") && strings.Contains(r, "5.0%") {
			found = true
		}
	}
	if !found {
		t.Fatalf("expected an interrupt-rate reason reporting 5.0%% (1/20, not 1/25=4.0%%), got reasons=%v", reasons)
	}
}

func TestVerdictDemandsFallbackWhenDatagramsAreInterrupted(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通"}
	var rs []Report
	for i := 0; i < 12; i++ {
		// 12 个里 2 个被中断 = 16.7%，超过 5% 的阈值
		rs = append(rs, reportWith(carriers[i%3], true, 0.3, 0.2, i < 2))
	}
	need, conclusive, reasons := Verdict(rs)
	if !need {
		t.Fatal("2 of 12 sessions interrupted is over the 5% threshold and must demand a fallback")
	}
	if !conclusive {
		t.Fatalf("this sample is adequate throughout; Verdict must be conclusive; reasons=%v", reasons)
	}
	// 光断言 need 证明不了本行触发了：这批样本的 stream 轮都没被中断，
	// 所以第四行(datagram 被中断而 stream 没有)也会同时越线，need 会
	// 因为它而为真。要证明的是第二行自己，所以断言第二行自己的措辞——
	// "过了握手的会话"这半句只有第二行有，第四行说的是"在多少比例的会话里"。
	found := false
	for _, r := range reasons {
		if strings.Contains(r, "of sessions that passed the handshake") {
			found = true
		}
	}
	if !found {
		t.Fatalf("the interrupt-rate row must fire in its own right, got reasons=%v", reasons)
	}
}

// TestVerdictDatagramOnlyInterruptDoesNotFireWhenBothRoundsAreInterrupted 证明
// 判定表第二行(被中断的比例)和替换后的第四行(datagram 被中断但 stream
// 没有)不是同一个计算——一条连接如果两轮都被掐断，说明这台网络对长连接
// 本身不友好，不是"中间设备在挑 datagram 特殊对待"，stream 回退救不了它，
// 所以不该计入第四行。
func TestVerdictDatagramOnlyInterruptDoesNotFireWhenBothRoundsAreInterrupted(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通"}
	var rs []Report
	for i := 0; i < 12; i++ {
		r := reportWith(carriers[i%3], true, 0.3, 0.2, i < 3) // 3/12 datagram 被中断 = 25%
		if i < 3 {
			r.Stream.Interrupted = true // ……stream 也被掐断，不是只有 datagram
		}
		rs = append(rs, r)
	}
	need, conclusive, reasons := Verdict(rs)
	if !need {
		t.Fatalf("3 of 12 sessions with the datagram round interrupted is over the row-2 threshold; reasons=%v", reasons)
	}
	if !conclusive {
		t.Fatalf("this sample is adequate throughout; Verdict must be conclusive; reasons=%v", reasons)
	}
	for _, r := range reasons {
		if strings.Contains(r, "on the same connection") {
			t.Fatalf("both rounds being interrupted together must not count toward the datagram-only-rescue metric, got reasons=%v", reasons)
		}
	}
}

// TestVerdictDemandsFallbackWhenMedianDatagramLossIsHigh 单独钉住判定表第三行：
// datagram 丢包率中位数 > 2%。12 个会话丢包率都是 5.0%，中位数自然是
// 5.0%，同时没有任何会话被中断，不会碰到其余三行。
func TestVerdictDemandsFallbackWhenMedianDatagramLossIsHigh(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通"}
	var rs []Report
	for i := 0; i < 12; i++ {
		rs = append(rs, reportWith(carriers[i%3], true, 5.0, 0.2, false))
	}
	need, conclusive, reasons := Verdict(rs)
	if !need {
		t.Fatalf("median datagram loss of 5.0%% is over the 2%% threshold; reasons=%v", reasons)
	}
	if !conclusive {
		t.Fatalf("this sample is adequate throughout; Verdict must be conclusive; reasons=%v", reasons)
	}
	found := false
	for _, r := range reasons {
		if strings.Contains(r, "median datagram loss") {
			found = true
		}
	}
	if !found {
		t.Fatalf("expected a median-datagram-loss reason, got %v", reasons)
	}
}

// TestVerdictDemandsFallbackWhenDatagramInterruptedButStreamIsNot 钉住 Ruling 2
// 的替换指标：datagram 轮被中断、但同一条连接上的 stream 轮没有被中断的
// 会话占比。同时用大量带 SendFailures 的会话验证 Ruling 5 的排除确实生效——
// 那 15 个会话的丢包率被故意设成很难看的 30%，如果没被排除会直接把中位数
// 推过阈值，但断言里中位数仍然干净，说明它们真的被拿掉了。usable 集合
// (1 个救回 + 9 个干净 = 10 个)刻意留在 minSessions 门槛之上，这样这个
// 测试单独钉住的是第四行，而不会被"修复轮 1"新加的排除后充分性检查挡在
// 门外——那个检查有自己专门的测试。
func TestVerdictDemandsFallbackWhenDatagramInterruptedButStreamIsNot(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通"}
	var rs []Report
	// 1 个会话：datagram 轮被中断，stream 轮没有——新指标要抓的那个人。
	rs = append(rs, reportWith(carriers[0], true, 8.3, 0.2, true))
	// 9 个干净会话，让 usable 停在 10 个(>= minSessions)，且覆盖全部
	// 3 家运营商(>= minCarriers)。
	for i := 0; i < 9; i++ {
		rs = append(rs, reportWith(carriers[i%3], true, 0.3, 0.2, false))
	}
	// 15 个发送失败的会话，数值故意设得很难看，用来同时验证它们确实被
	// 从分析里排除了。
	for i := 0; i < 15; i++ {
		r := reportWith(carriers[i%3], true, 30.0, 0.2, false)
		r.Datagram.SendFailures = 3
		rs = append(rs, r)
	}

	need, conclusive, reasons := Verdict(rs)
	if !conclusive {
		t.Fatalf("10 usable sessions across 3 carriers is adequate; Verdict must be conclusive; reasons=%v", reasons)
	}
	if !need {
		t.Fatalf("1 of 10 usable sessions rescued by the stream round is over the 5%% threshold; reasons=%v", reasons)
	}
	var foundRow4, foundExclusion, foundBadMedian bool
	for _, r := range reasons {
		if strings.Contains(r, "on the same connection") {
			foundRow4 = true
		}
		if strings.Contains(r, "excluded 15 session") {
			foundExclusion = true
		}
		if strings.Contains(r, "median datagram loss") {
			foundBadMedian = true
		}
	}
	if !foundRow4 {
		t.Fatalf("expected the datagram-only-interrupt reason, got reasons=%v", reasons)
	}
	if !foundExclusion {
		t.Fatalf("expected Verdict to report how many send-failure sessions were excluded, got reasons=%v", reasons)
	}
	if foundBadMedian {
		t.Fatalf("the 15 send-failure sessions (30%% loss) must not leak into the median-loss threshold, got reasons=%v", reasons)
	}
}

// TestVerdictExcludesRoundsThatNeverRanFromComparison 钉住 Ruling 4：一个
// Stream.Failed 的会话，它的 Stream.LossPercent/Interrupted 都停在零值——
// 不是"stream 轮完美地没有被中断"。2 个这样的会话同时 datagram 轮被中断，
// 如果不排除，会被新指标误读成"stream 救回了他们"；本测试断言这类会话
// 不计入新指标的分子，即使它们仍然合理地计入了第二行(datagram 被中断)。
func TestVerdictExcludesRoundsThatNeverRanFromComparison(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通"}
	var rs []Report
	for i := 0; i < 12; i++ {
		if i < 2 {
			r := reportWith(carriers[i%3], true, 8.3, 0, true)
			r.Stream = RoundResult{Failed: true, FirstLossAtSecond: -1, Error: "test: stream never opened"}
			rs = append(rs, r)
			continue
		}
		rs = append(rs, reportWith(carriers[i%3], true, 0.3, 0.2, false))
	}

	need, conclusive, reasons := Verdict(rs)
	if !conclusive {
		t.Fatalf("10 usable sessions across 3 carriers is adequate; Verdict must be conclusive; reasons=%v", reasons)
	}
	if !need {
		t.Fatalf("2 of 12 sessions with the datagram round interrupted is over the row-2 threshold; reasons=%v", reasons)
	}
	foundRow2 := false
	for _, r := range reasons {
		if strings.Contains(r, "on the same connection") {
			t.Fatalf("a session whose stream round never ran must not count as a datagram-only-rescue case, got reasons=%v", reasons)
		}
		if strings.Contains(r, "sessions that passed the handshake") {
			foundRow2 = true
		}
	}
	if !foundRow2 {
		t.Fatalf("expected the plain datagram-interrupt-rate reason to still fire, got reasons=%v", reasons)
	}
}

// TestVerdictExcludesSendFailureSessionsAndReportsCountWithoutForcingAVerdict
// 钉住 Ruling 5 的另一半，以及修复轮 1 要买的东西："有诊断说明"和"结论
// 不成立"必须彻底分开。11 个干净会话 + 1 个带 SendFailures 的会话，排除
// 之后剩下 11 个仍然充足(>= minSessions)、仍然覆盖 3 家运营商
// (>= minCarriers)、仍然干净，不该触发任何阈值——但 reasons 里必须能
// 看到"排除了 1 个"的说明，同时 conclusive 必须是 true、need 必须是
// false。
func TestVerdictExcludesSendFailureSessionsAndReportsCountWithoutForcingAVerdict(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通"}
	var rs []Report
	for i := 0; i < 11; i++ {
		rs = append(rs, reportWith(carriers[i%3], true, 0.3, 0.2, false))
	}
	suspect := reportWith(carriers[0], true, 0.3, 0.2, false)
	suspect.Datagram.SendFailures = 1
	rs = append(rs, suspect)

	need, conclusive, reasons := Verdict(rs)
	if !conclusive {
		t.Fatalf("11 usable sessions across 3 carriers is adequate; Verdict must be conclusive; reasons=%v", reasons)
	}
	if need {
		t.Fatalf("excluding one suspect session must not by itself demand a fallback; reasons=%v", reasons)
	}
	found := false
	for _, r := range reasons {
		if strings.Contains(r, "excluded 1 session") {
			found = true
		}
	}
	if !found {
		t.Fatalf("Verdict must report how many send-failure sessions were excluded, got reasons=%v", reasons)
	}
}

// TestVerdictRefusesWhenNoUsableSessionsRemain 钉住修复轮 1 吸收掉的
// Ruling 6 特例：全部 12 个会话的 stream 轮都没跑起来，排除之后可用样本
// 是空集(0 < minSessions)。必须像样本太少、运营商太少那样明确拒绝下
// 结论(conclusive=false)，而不是让空切片的默认值悄悄读成"不需要回退"。
func TestVerdictRefusesWhenNoUsableSessionsRemain(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通"}
	var rs []Report
	for i := 0; i < 12; i++ {
		r := reportWith(carriers[i%3], true, 0.3, 0, false)
		r.Stream = RoundResult{Failed: true, FirstLossAtSecond: -1, Error: "test: stream never opened"}
		rs = append(rs, r)
	}
	_, conclusive, reasons := Verdict(rs)
	if conclusive {
		t.Fatalf("an empty usable set must not be read as conclusive either way; reasons=%v", reasons)
	}
	if len(reasons) == 0 {
		t.Fatal("Verdict must refuse to conclude when no usable sessions remain, not silently say no fallback is needed")
	}
}

// TestVerdictBecomesInconclusiveWhenExclusionsDropBelowMinSessions 钉住
// 修复轮 1 的核心：样本充足性此前只在排除之前检查过一次。10 个会话、
// 3 家运营商，过了顶部的门槛；其中 6 个因 SendFailures 被排除，usable
// 只剩 4 个——如果不重新检查，这 4 个样本的丢包率中位数会去决定一个
// 要维护多年的传输层。理由里必须同时出现排除前("10")和排除后("4")
// 的数字，否则读的人会以为一开始就没收够。
func TestVerdictBecomesInconclusiveWhenExclusionsDropBelowMinSessions(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通"}
	var rs []Report
	for i := 0; i < 10; i++ {
		r := reportWith(carriers[i%3], true, 0.3, 0.2, false)
		if i < 6 {
			r.Datagram.SendFailures = 1 // 6 个被排除，usable 只剩 4 个
		}
		rs = append(rs, r)
	}
	need, conclusive, reasons := Verdict(rs)
	if conclusive {
		t.Fatalf("4 usable sessions (after excluding 6 of 10) is below minSessions; Verdict must refuse, got need=%v reasons=%v", need, reasons)
	}
	found := false
	for _, r := range reasons {
		if strings.Contains(r, "4 of 10") && strings.Contains(r, "usable") {
			found = true
		}
	}
	if !found {
		t.Fatalf("expected a reason naming both the before (10) and after (4) counts, got reasons=%v", reasons)
	}
}

// TestVerdictBecomesInconclusiveWhenExclusionsDropBelowMinCarriers 钉住
// 修复轮 1 的另一半：排除后会话数仍然够(>= minSessions)，但全部剩下的
// 会话只来自一家运营商。20 个会话覆盖 3 家运营商，其中一家的 10 个全部
// 干净(usable)，另外两家的 10 个全部因 SendFailures 被排除——usable
// 还剩 10 个(足够)，但只剩 1 家运营商，覆盖面等同于单一样本点。
func TestVerdictBecomesInconclusiveWhenExclusionsDropBelowMinCarriers(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通"}
	var rs []Report
	for i := 0; i < 10; i++ {
		rs = append(rs, reportWith(carriers[0], true, 0.3, 0.2, false)) // 电信：干净，留在 usable
	}
	for i := 0; i < 5; i++ {
		r := reportWith(carriers[1], true, 0.3, 0.2, false)
		r.Datagram.SendFailures = 1
		rs = append(rs, r) // 移动：全部排除
	}
	for i := 0; i < 5; i++ {
		r := reportWith(carriers[2], true, 0.3, 0.2, false)
		r.Datagram.SendFailures = 1
		rs = append(rs, r) // 联通：全部排除
	}

	need, conclusive, reasons := Verdict(rs)
	if conclusive {
		t.Fatalf("usable sessions all coming from 1 carrier is below minCarriers; Verdict must refuse, got need=%v reasons=%v", need, reasons)
	}
	found := false
	for _, r := range reasons {
		if strings.Contains(r, "1 carrier(s) among 10") {
			found = true
		}
	}
	if !found {
		t.Fatalf("expected a reason naming the narrowed carrier count among the usable sessions, got reasons=%v", reasons)
	}
}
