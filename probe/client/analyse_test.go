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
	v := Verdict(rs)
	if v.Conclusive {
		t.Fatalf("Verdict must refuse below 8 sessions, got conclusive=true notes=%v", v.Notes)
	}
	if len(v.Notes) == 0 || v.Notes[0][:6] != "sample" {
		t.Fatalf("Verdict must refuse below 8 sessions, got notes=%v", v.Notes)
	}
}

// TestVerdictRefusesToDecideOnTooFewNetworks 钉住终审 Finding B 措辞变化后的
// 门槛本身：Carrier 是自由文本，这条门槛真正衡量的是"覆盖了几个不同的网络"，
// 不是"覆盖了几家运营商"——所以拒绝的理由里现在说 networks，不说 carriers。
func TestVerdictRefusesToDecideOnTooFewNetworks(t *testing.T) {
	var rs []Report
	for i := 0; i < 10; i++ {
		rs = append(rs, reportWith("中国电信", true, 0, 0, false))
	}
	v := Verdict(rs)
	if v.Conclusive {
		t.Fatalf("Verdict must refuse with fewer than 3 networks, got conclusive=true notes=%v", v.Notes)
	}
	found := false
	for _, n := range v.Notes {
		if strings.Contains(n, "networks") {
			found = true
		}
		if strings.Contains(n, "carriers") {
			t.Fatalf("wording must say networks, not carriers (Carrier is free text and does not guarantee carrier diversity), got %v", v.Notes)
		}
	}
	if !found {
		t.Fatalf("Verdict must refuse with fewer than 3 networks, got notes=%v", v.Notes)
	}
}

func TestVerdictSaysNoFallbackWhenEverythingIsClean(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通", "校园网"}
	var rs []Report
	for i := 0; i < 12; i++ {
		rs = append(rs, reportWith(carriers[i%len(carriers)], true, 0.3, 0.2, false))
	}
	v := Verdict(rs)
	if v.NeedFallback {
		t.Fatalf("clean data must not demand a fallback; crossed=%v findings=%v", v.Crossed, v.Findings)
	}
	if !v.Conclusive {
		t.Fatalf("a fully adequate, clean sample must be conclusive; notes=%v", v.Notes)
	}
	if len(v.Notes) != 0 || len(v.Crossed) != 0 || len(v.Findings) != 0 {
		t.Fatalf("a clean sample with no exclusions must produce nothing in any category, got notes=%v crossed=%v findings=%v",
			v.Notes, v.Crossed, v.Findings)
	}
}

// TestVerdictHandshakeFailureRateIsAFindingNotACross 钉住终审的统一裁决：
// 握手失败拿不到 stream 回退——回退活在同一条 QUIC 连接内部，连接都没建立
// 起来就没有地方给它活。2/20 = 10%，远高于 2% 的报告阈值，其余全部干净，
// 不碰其它任何一行；必须出现在 Findings 里，绝不能出现在 Crossed 里，
// 也绝不能把 NeedFallback 拨成 true。
func TestVerdictHandshakeFailureRateIsAFindingNotACross(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通"}
	var rs []Report
	for i := 0; i < 2; i++ {
		rs = append(rs, reportWith(carriers[i%3], false, 0, 0, false))
	}
	for i := 0; i < 18; i++ {
		rs = append(rs, reportWith(carriers[i%3], true, 0.3, 0.2, false))
	}
	v := Verdict(rs)
	if !v.Conclusive {
		t.Fatalf("this sample is adequate throughout; Verdict must be conclusive; notes=%v", v.Notes)
	}
	if v.NeedFallback {
		t.Fatalf("a handshake failure rate must NOT by itself demand a fallback; crossed=%v findings=%v", v.Crossed, v.Findings)
	}
	if len(v.Crossed) != 0 {
		t.Fatalf("handshake failure rate must never appear in Crossed, got %v", v.Crossed)
	}
	found := false
	for _, f := range v.Findings {
		if strings.Contains(f, "handshake failed") {
			found = true
		}
	}
	if !found {
		t.Fatalf("expected a handshake-failure finding, got findings=%v", v.Findings)
	}
}

// TestVerdictDemandsFallbackWhenDatagramInterruptedButStreamIsNot 钉住判定表
// 剩下两行里唯一衡量"中断"的那一行：datagram 轮被中断、但同一条连接上的
// stream 轮没有被中断的会话占比。同时用大量带 SendFailures 的会话验证
// Ruling 5 的排除确实生效——那 15 个会话的丢包率被故意设成很难看的 30%，
// 如果没被排除会直接把中位数推过阈值，但断言里中位数仍然干净，说明它们
// 真的被拿掉了。usable 集合(1 个救回 + 9 个干净 = 10 个)刻意留在
// minSessions 门槛之上，这样这个测试单独钉住的是这一行，而不会被排除后
// 充分性检查挡在门外——那个检查有自己专门的测试。断言 Crossed 恰好只有
// 这一条，证明它是"单独"触发的，没有其它行混进来凑数。
func TestVerdictDemandsFallbackWhenDatagramInterruptedButStreamIsNot(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通"}
	var rs []Report
	// 1 个会话：datagram 轮被中断，stream 轮没有——这一行要抓的那个人。
	rs = append(rs, reportWith(carriers[0], true, 8.3, 0.2, true))
	// 9 个干净会话，让 usable 停在 10 个(>= minSessions)，且覆盖全部
	// 3 个网络(>= minCarriers)。
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

	v := Verdict(rs)
	if !v.Conclusive {
		t.Fatalf("10 usable sessions across 3 networks is adequate; Verdict must be conclusive; notes=%v", v.Notes)
	}
	if !v.NeedFallback {
		t.Fatalf("1 of 10 usable sessions rescued by the stream round is over the 5%% threshold; crossed=%v", v.Crossed)
	}
	if len(v.Crossed) != 1 || !strings.Contains(v.Crossed[0], "on the same connection") {
		t.Fatalf("this row must fire alone, got crossed=%v", v.Crossed)
	}
	foundExclusion := false
	for _, n := range v.Notes {
		if strings.Contains(n, "excluded 15 session") {
			foundExclusion = true
		}
	}
	if !foundExclusion {
		t.Fatalf("expected Verdict to report how many send-failure sessions were excluded, got notes=%v", v.Notes)
	}
}

// TestVerdictBothRoundsInterruptedIsAFindingNotACross 钉住终审统一裁决的
// 另一半：一条连接如果两轮都被掐断，说明这台网络对长连接本身不友好，
// 不是"中间设备在挑 datagram 特殊对待"，stream 回退救不了它。2/12 = 16.7%，
// 远高于报告阈值，其余全部干净；必须出现在 Findings 里，绝不能出现在
// Crossed 里，也绝不能把 NeedFallback 拨成 true。
func TestVerdictBothRoundsInterruptedIsAFindingNotACross(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通"}
	var rs []Report
	for i := 0; i < 12; i++ {
		r := reportWith(carriers[i%3], true, 0.3, 0.2, i < 2) // 2/12 datagram 被中断
		if i < 2 {
			r.Stream.Interrupted = true // ……stream 也被掐断
		}
		rs = append(rs, r)
	}
	v := Verdict(rs)
	if !v.Conclusive {
		t.Fatalf("this sample is adequate throughout; Verdict must be conclusive; notes=%v", v.Notes)
	}
	if v.NeedFallback {
		t.Fatalf("both rounds interrupted together must NOT by itself demand a fallback (the connection itself died); crossed=%v findings=%v", v.Crossed, v.Findings)
	}
	if len(v.Crossed) != 0 {
		t.Fatalf("both-rounds-interrupted rate must never appear in Crossed, got %v", v.Crossed)
	}
	found := false
	for _, f := range v.Findings {
		if strings.Contains(f, "both rounds were interrupted") {
			found = true
		}
	}
	if !found {
		t.Fatalf("expected a both-rounds-interrupted finding, got findings=%v", v.Findings)
	}
}

// TestVerdictDemandsFallbackWhenMedianDatagramLossIsHigh 单独钉住判定表另一
// 行：datagram 丢包率中位数 > 2%。12 个会话丢包率都是 5.0%，中位数自然是
// 5.0%，同时没有任何会话被中断、没有握手失败，不会碰到其它任何一行——
// 断言 Crossed 恰好只有这一条。
func TestVerdictDemandsFallbackWhenMedianDatagramLossIsHigh(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通"}
	var rs []Report
	for i := 0; i < 12; i++ {
		rs = append(rs, reportWith(carriers[i%3], true, 5.0, 0.2, false))
	}
	v := Verdict(rs)
	if !v.NeedFallback {
		t.Fatalf("median datagram loss of 5.0%% is over the 2%% threshold; crossed=%v", v.Crossed)
	}
	if !v.Conclusive {
		t.Fatalf("this sample is adequate throughout; Verdict must be conclusive; notes=%v", v.Notes)
	}
	if len(v.Crossed) != 1 || !strings.Contains(v.Crossed[0], "median datagram loss") {
		t.Fatalf("this row must fire alone, got crossed=%v", v.Crossed)
	}
	if len(v.Findings) != 0 {
		t.Fatalf("this scenario must not trip either separate finding, got findings=%v", v.Findings)
	}
}

// TestVerdictExcludesRoundsThatNeverRanFromComparison 钉住 Ruling 4：一个
// Stream.Failed 的会话，它的 Stream.LossPercent/Interrupted 都停在零值——
// 不是"stream 轮完美地没有被中断"。2 个这样的会话同时 datagram 轮被中断，
// 如果不排除，会被误读成"stream 救回了他们"；本测试断言这类会话完全被
// 排除出 usable，既不计入救回行，也不该让整体结论跑偏。
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

	v := Verdict(rs)
	if !v.Conclusive {
		t.Fatalf("10 usable sessions across 3 networks is adequate; Verdict must be conclusive; notes=%v", v.Notes)
	}
	if v.NeedFallback {
		t.Fatalf("2 sessions whose stream round never ran must not push NeedFallback to true; crossed=%v", v.Crossed)
	}
	for _, c := range v.Crossed {
		if strings.Contains(c, "on the same connection") {
			t.Fatalf("a session whose stream round never ran must not count as a datagram-only-rescue case, got crossed=%v", v.Crossed)
		}
	}
}

// TestVerdictExcludesSendFailureSessionsAndReportsCountWithoutForcingAVerdict
// 钉住 Ruling 5 的另一半："有诊断说明"和"结论不成立"必须彻底分开。11 个
// 干净会话 + 1 个带 SendFailures 的会话，排除之后剩下 11 个仍然充足
// (>= minSessions)、仍然覆盖 3 个网络(>= minCarriers)、仍然干净，不该
// 触发任何一行——但 Notes 里必须能看到"排除了 1 个"的说明，同时
// Conclusive 必须是 true、NeedFallback 必须是 false。
func TestVerdictExcludesSendFailureSessionsAndReportsCountWithoutForcingAVerdict(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通"}
	var rs []Report
	for i := 0; i < 11; i++ {
		rs = append(rs, reportWith(carriers[i%3], true, 0.3, 0.2, false))
	}
	suspect := reportWith(carriers[0], true, 0.3, 0.2, false)
	suspect.Datagram.SendFailures = 1
	rs = append(rs, suspect)

	v := Verdict(rs)
	if !v.Conclusive {
		t.Fatalf("11 usable sessions across 3 networks is adequate; Verdict must be conclusive; notes=%v", v.Notes)
	}
	if v.NeedFallback {
		t.Fatalf("excluding one suspect session must not by itself demand a fallback; crossed=%v", v.Crossed)
	}
	found := false
	for _, n := range v.Notes {
		if strings.Contains(n, "excluded 1 session") {
			found = true
		}
	}
	if !found {
		t.Fatalf("Verdict must report how many send-failure sessions were excluded, got notes=%v", v.Notes)
	}
}

// TestVerdictRefusesWhenNoUsableSessionsRemain 钉住 Ruling 6 的特例：全部
// 12 个会话的 stream 轮都没跑起来，排除之后可用样本是空集(0 < minSessions)。
// 必须像样本太少、网络太少那样明确拒绝下结论(Conclusive=false)，而不是让
// 空切片的默认值悄悄读成"不需要回退"。
func TestVerdictRefusesWhenNoUsableSessionsRemain(t *testing.T) {
	carriers := []string{"中国电信", "中国移动", "中国联通"}
	var rs []Report
	for i := 0; i < 12; i++ {
		r := reportWith(carriers[i%3], true, 0.3, 0, false)
		r.Stream = RoundResult{Failed: true, FirstLossAtSecond: -1, Error: "test: stream never opened"}
		rs = append(rs, r)
	}
	v := Verdict(rs)
	if v.Conclusive {
		t.Fatalf("an empty usable set must not be read as conclusive either way; notes=%v", v.Notes)
	}
	if len(v.Notes) == 0 {
		t.Fatal("Verdict must refuse to conclude when no usable sessions remain, not silently say no fallback is needed")
	}
}

// TestVerdictBecomesInconclusiveWhenExclusionsDropBelowMinSessions 钉住
// 修复轮 1 的核心：样本充足性此前只在排除之前检查过一次。10 个会话、
// 3 个网络，过了顶部的门槛；其中 6 个因 SendFailures 被排除，usable
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
	v := Verdict(rs)
	if v.Conclusive {
		t.Fatalf("4 usable sessions (after excluding 6 of 10) is below minSessions; Verdict must refuse, got need=%v notes=%v", v.NeedFallback, v.Notes)
	}
	found := false
	for _, n := range v.Notes {
		if strings.Contains(n, "4 of 10") && strings.Contains(n, "usable") {
			found = true
		}
	}
	if !found {
		t.Fatalf("expected a note naming both the before (10) and after (4) counts, got notes=%v", v.Notes)
	}
}

// TestVerdictBecomesInconclusiveWhenExclusionsDropBelowMinNetworks 钉住
// 修复轮 1 的另一半：排除后会话数仍然够(>= minSessions)，但全部剩下的
// 会话只来自一个网络。20 个会话覆盖 3 个网络，其中一个的 10 个全部干净
// (usable)，另外两个的 10 个全部因 SendFailures 被排除——usable 还剩
// 10 个(足够)，但只剩 1 个网络，覆盖面等同于单一样本点。措辞用
// "network(s)"，不用"carrier(s)"(终审 Finding B)。
func TestVerdictBecomesInconclusiveWhenExclusionsDropBelowMinNetworks(t *testing.T) {
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

	v := Verdict(rs)
	if v.Conclusive {
		t.Fatalf("usable sessions all coming from 1 network is below minCarriers; Verdict must refuse, got need=%v notes=%v", v.NeedFallback, v.Notes)
	}
	found := false
	for _, n := range v.Notes {
		if strings.Contains(n, "1 network(s) among 10") {
			found = true
		}
		if strings.Contains(n, "carrier") {
			t.Fatalf("wording must say network(s), not carrier(s), got %v", v.Notes)
		}
	}
	if !found {
		t.Fatalf("expected a note naming the narrowed network count among the usable sessions, got notes=%v", v.Notes)
	}
}

// TestNormaliseCarrierMergesWhitespaceAndCaseVariants 钉住终审 Finding B 的
// 归一化那一半：同一个网络仅仅因为前后空白或大小写不同被打进两个不同的桶，
// 会让覆盖面门槛形同虚设。这里只做 trim + 大小写折叠，不做真正的运营商
// 别名识别(那需要一张表，超出这个一次性工具该做的事——见 normaliseCarrier
// 的注释)。
func TestNormaliseCarrierMergesWhitespaceAndCaseVariants(t *testing.T) {
	cases := [][2]string{
		{"China Telecom", "china telecom"},
		{" 中国电信", "中国电信"},
		{"中国电信 ", "中国电信"},
	}
	for _, c := range cases {
		if normaliseCarrier(c[0]) != normaliseCarrier(c[1]) {
			t.Fatalf("normaliseCarrier(%q)=%q must equal normaliseCarrier(%q)=%q",
				c[0], normaliseCarrier(c[0]), c[1], normaliseCarrier(c[1]))
		}
	}
}

// 样本构成要在**样本还不够的时候**就看得见——那正是最需要它的时刻。
//
// 只说"还差 3 份"和说清楚"还差 3 份、而且联通一个都没有"，对招募是两件
// 完全不同的事，而 README-部署.md 那张表里"大陆三家运营商各 ≥ 1"这一行
// 代码管不了，只能靠人对着看。
func TestCoverageIsReportedEvenWhenTheSampleIsTooSmall(t *testing.T) {
	rs := []Report{
		{OS: "windows", Carrier: "中国电信", Handshake: HandshakeResult{OK: true}},
		{OS: "windows", Carrier: " 中国电信 ", Handshake: HandshakeResult{OK: true}},
		{OS: "darwin", Carrier: "校园网", Handshake: HandshakeResult{OK: true}},
	}
	v := Verdict(rs)
	if v.Conclusive {
		t.Fatalf("3 sessions is below the threshold; Verdict must refuse")
	}
	if len(v.Coverage) != 2 {
		t.Fatalf("coverage must carry a network line and an os line, got %v", v.Coverage)
	}
	joined := strings.Join(v.Coverage, " | ")
	// 大小写和前后空白不同不该数成两个网络——那会让人以为覆盖面比实际更宽。
	if !strings.Contains(joined, "中国电信 ×2") {
		t.Fatalf("carriers differing only by whitespace must be one group, got %q", joined)
	}
	if !strings.Contains(joined, "2 network(s)") {
		t.Fatalf("expected 2 networks, got %q", joined)
	}
	if !strings.Contains(joined, "windows ×2") || !strings.Contains(joined, "darwin ×1") {
		t.Fatalf("expected an os breakdown, got %q", joined)
	}
}

// 构成是给招募看的，**不驱动判定**——所以它和 Notes 分开装，
// 一个干净且足量的样本仍然不该有任何 Notes。
func TestCoverageDoesNotLeakIntoTheDiagnostics(t *testing.T) {
	var rs []Report
	for i := 0; i < 9; i++ {
		rs = append(rs, Report{
			OS:        "windows",
			Carrier:   []string{"电信", "移动", "联通"}[i%3],
			Handshake: HandshakeResult{OK: true},
			Datagram:  RoundResult{Sent: 3000, Received: 3000, FirstLossAtSecond: -1},
			Stream:    RoundResult{Sent: 3000, Received: 3000, FirstLossAtSecond: -1},
		})
	}
	v := Verdict(rs)
	if !v.Conclusive {
		t.Fatalf("this sample is adequate; notes=%v", v.Notes)
	}
	if len(v.Notes) != 0 {
		t.Fatalf("coverage must not be filed as a diagnostic, got notes=%v", v.Notes)
	}
	if len(v.Coverage) != 2 {
		t.Fatalf("coverage must still be filled on the conclusive path, got %v", v.Coverage)
	}
}
