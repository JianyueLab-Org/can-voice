// can-voice-probe analyse 汇总测试用户回传的报告并给出判定。
// 判定标准写死在下面的常量里，且在看到任何数据之前就已经定下——
// 见 P1 计划 Task 9。
//
// 终审（读完整个分支之后的复查）改动了这个文件的判定表本身，不只是
// 措辞：needFallback 现在只回答一个问题——"如果做了 stream 回退，这个
// 人本可以被救回来吗"。握手失败和"两轮连接本身都被掐断"两类会话答案
// 都是否定的（回退是同一条 QUIC 连接内部的一个通道，连接都没建立起来
// 或者已经死了，回退没有地方活），所以它们从驱动 needFallback 的
// Crossed 里挪出去，变成两个独立的 Findings——见 VerdictResult 的注释。
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

const (
	minSessions          = 8
	minCarriers          = 3
	maxHandshakeFailRate = 0.02 // 2%
	maxDatagramLoss      = 2.0  // 百分比

	// maxDatagramOnlyInterruptRate 替换了 Task 9 判定表原来的第四行
	// ("datagram 与 stream 丢包率之差的中位数 > 2 个百分点")。
	//
	// 那一行在算术上和第三行(datagram 丢包率中位数 > 2%)永远同时成立：
	// QUIC 的 stream 可靠有序，协议自己会重传，round.go 里
	// RunStreamRound 的注释也写了 loss≈0 是这一轮的预期结果，不是需要修
	// 的 bug。既然 stream 丢包率恒为 ~0，"两轮丢包率之差"就几乎等于
	// "datagram 丢包率"本身——两行只是同一个信号写了两遍。真的越过
	// 阈值时 reasons 会同时列出两条，读起来像是两个独立证据互相印证，
	// 其实只是同一件事被数了两遍；对一份要支撑多年传输层决策的文档，
	// 这是实质性的误导。
	//
	// 这不是"看了数据之后挪阈值"——此刻一条数据都还没有，问题出在这个
	// 指标本身在协议的构造下就立不住，而不是数据碰巧长这样。换成它本该
	// 回答的问题：如果做了 stream 回退，这个人本可以被救回来吗——也就是
	// datagram 轮被中断、但同一条连接上的 stream 轮没有被中断的会话
	// 占比。阈值沿用"被中断"那一行(row 2)的 5%。
	maxDatagramOnlyInterruptRate = 0.05 // 5%

	// maxBothInterruptedRate 是终审新增的第二个独立结论("两轮都被中断
	// 的会话占比")的报告阈值。这个数值不是看了数据之后现造的——它就是
	// 判定表原来"datagram 轮被中断"那一行(row 2，denominator 用"过了
	// 握手的会话数")用过的 5%。那一行现在整个拿掉了：它和
	// maxDatagramOnlyInterruptRate 那一行几乎在同一批会话上触发(stream
	// 轮被中断本来就是按设计很罕见的事)，reasons 里印两条读起来像两个
	// 独立证据互相印证，其实是同一件事数了两遍——这正是当初把"丢包率
	// 之差"那一行换掉的同一个理由，只是这次换掉的是"被中断"那一行。
	// 但"两轮都被中断"和"只有 datagram 被中断"衡量的是两个不同的问题
	// (前者是"这条连接本身死了"，回退救不了；后者是"stream 回退能救回
	// 这个人")，所以它没有被直接删掉，而是保留下来作为独立结论，复用
	// 已经定过的这个数字，而不是现在看着数据挑一个新的。
	maxBothInterruptedRate = 0.05 // 5%
)

// normaliseCarrier 把 -carrier 的自由文本做最基本的归一化，只用于判断
// "这批样本覆盖了几个不同的网络"这一件事——不做真正的运营商识别，那需要
// 一张别名表，超出这个一次性工具该做的事。三个人分别填
// "中国电信"/" 中国电信 "/"CHINA TELECOM" 不该仅仅因为大小写或前后空白
// 不同就被数成三个不同的网络。
func normaliseCarrier(s string) string {
	return strings.ToLower(strings.TrimSpace(s))
}

// VerdictResult 是 Verdict 的返回值。
//
// 终审发现旧版本把四种不同角色的信息揉进一个 (bool, bool, []string)：
// 真正驱动"要不要做 stream 回退"的理由、纯粹的诊断性说明、以及两个完全
// 独立、不该驱动回退结论的发现。旧版本的 analyseMain 把前两种印成同一种
// "  - %s"，读的人分不出谁是谁；更糟的是诊断说明把 Conclusive 拨成 false
// 之后，已经收集到的越线理由还会印在"VERDICT: inconclusive"上面——读起来
// 像"越过了阈值，但没有结论"，自相矛盾。现在四个字段分开装，谁都不用去猜，
// 而且 Crossed 只在函数末尾统一裁决 Conclusive=true 的那条路径上才会被
// 写入，结构上就不可能出现"越线理由印在 inconclusive 上面"这种矛盾。
type VerdictResult struct {
	// Conclusive 说明这一次判定是否立得住(数据是否足够支撑它)。
	// NeedFallback 只在 Conclusive 为真时才有意义。
	Conclusive   bool
	NeedFallback bool

	// Crossed 是真正越过阈值、驱动 NeedFallback 的理由。终审之后判定表
	// 只剩两行会出现在这里：datagram 轮被中断而同一条连接上的 stream
	// 轮没有(maxDatagramOnlyInterruptRate)，以及 datagram 丢包率中位数
	// (maxDatagramLoss)——两者共同回答同一个问题："stream 回退能不能
	// 救回这些人"。
	Crossed []string

	// Notes 是诊断性说明：排除了多少可疑样本、排除后样本是否还够。
	// 纯说明，不驱动 NeedFallback，本身也不是一条独立结论。
	Notes []string

	// Findings 是终审新增的两个独立结论，即使数字很难看也不会让
	// NeedFallback 变成 true——因为 stream 回退救不了这两种情况：
	//   - 握手失败率：回退是同一条 QUIC 连接内部的一个通道，连接都没
	//     建立起来，回退没有地方活。这个数字回答的是另一个、更大的问题——
	//     QUIC 这条路能不能走通，答不出来的话需要的是换传输层，不是
	//     连接内部的一个回退通道。
	//   - 两轮都被中断的会话占比：连接建立起来了，但后来死了，同样没有
	//     回退可以活的地方。
	// analyseMain 必须把这两条和上面的 Crossed/VERDICT 分开、显眼地印
	// 出来——这份输出会被原样抄进最终留存的文档，握手失败的发现绝不能
	// 被读成"需要回退通道"的证据。
	Findings []string
}

// Verdict 判定是否必须实现 stream 回退通道。
//
// 样本不足时拒绝出结论：用太少的数据决定一个要维护多年的传输层，
// 比没有数据更危险，因为它看起来像有依据——这条规矩不仅在排除之前
// 检查一次，排除之后也要重新检查一次（见下面 usable 那一段的注释），
// 否则"少于 8 个会话/少于 3 个网络就拒绝判定"这条门槛会被排除路径
// 悄悄绕过去。
func Verdict(rs []Report) VerdictResult {
	if len(rs) < minSessions {
		return VerdictResult{Notes: []string{fmt.Sprintf(
			"sample too small: %d sessions, need at least %d", len(rs), minSessions)}}
	}
	carriers := map[string]bool{}
	for _, r := range rs {
		carriers[normaliseCarrier(r.Carrier)] = true
	}
	if len(carriers) < minCarriers {
		// 措辞是"networks"不是"carriers"：Carrier 是自由文本，"校园网"/
		// "公司网络"/"家里的wifi"这类完全不指名运营商的填法一样会通过
		// 这道门槛，把这里叫"运营商数"会让人以为覆盖面比实际更具体。
		return VerdictResult{Notes: []string{fmt.Sprintf(
			"sample too narrow: %d networks, need at least %d", len(carriers), minCarriers)}}
	}

	var res VerdictResult
	cross := func(format string, args ...any) {
		res.Crossed = append(res.Crossed, fmt.Sprintf(format, args...))
	}
	note := func(format string, args ...any) {
		res.Notes = append(res.Notes, fmt.Sprintf(format, args...))
	}
	finding := func(format string, args ...any) {
		res.Findings = append(res.Findings, fmt.Sprintf(format, args...))
	}

	// 握手失败的会话两轮根本没有机会开始，不该被当成"跑完了但很干净"
	// 计入下面几行；hsFail 的分母仍然是全部会话——它衡量的正是"有多少
	// 人连握手都过不去"，全部会话才是正确的分母。
	//
	// 终审：这不再是一条 Crossed 理由。stream 回退活在同一条 QUIC 连接
	// 内部，握手都没做完就没有连接可言，回退在这种会话里根本没有对象——
	// round.go("两轮都失败 = UDP 被整体阻断，stream 回退没有意义")和
	// 下面 Summarise 里的同一句话说的是同一件事。这里改成 finding：
	// 它回答的是"QUIC 这条路能不能走通"，而不是"需不需要在 QUIC 内部
	// 加一条回退通道"。
	hsFail := 0
	var passed []Report
	for _, r := range rs {
		if !r.Handshake.OK {
			hsFail++
			continue
		}
		passed = append(passed, r)
	}
	if rate := float64(hsFail) / float64(len(rs)); rate >= maxHandshakeFailRate {
		finding("handshake failed in %.1f%% of sessions (reporting threshold %.0f%%) — "+
			"a failed handshake has no connection for a stream fallback to live in; "+
			"this is a can-QUIC-be-used-at-all finding, not a fallback-channel finding",
			rate*100, maxHandshakeFailRate*100)
	}

	if len(passed) == 0 {
		// 全部会话都没握手成功，后面几行没有对象可算——上面的 finding
		// 已经把这个情况说清楚了。NeedFallback 保持零值 false：终审之后
		// 握手失败本身不能作为"需要回退"的理由，不管失败率有多高。
		res.Conclusive = true
		return res
	}

	// usable 是排除了"轮次根本没跑起来"和"发送本身在失败"两类会话之后
	// 剩下的集合，下面的丢包率中位数、datagram-only-interrupt 和
	// both-interrupted 都建立在它上面。
	var usable []Report
	suspect := 0
	for _, r := range passed {
		if r.Datagram.Failed || r.Stream.Failed {
			// 轮次根本没跑起来：它的 LossPercent/Interrupted 都停在零值，
			// 不是"跑完了、什么都没丢"的测量结果。拿它去算中位数或做
			// datagram-vs-stream 对比，就是把"没有数据"读成"数据很好"
			// (Ruling 4)——round.go 里 RoundResult.Failed 上的注释是
			// 同一件事。
			continue
		}
		if r.Datagram.SendFailures > 0 || r.Stream.SendFailures > 0 {
			// 发送本身在失败，这一轮的丢包率和 Interrupted 判定都可疑——
			// round.go 里 RoundResult.SendFailures 字段上的注释说的是
			// 同一个已知、可接受的局限。排除，但要计数而不是悄悄混进去
			// (Ruling 5)。
			suspect++
			continue
		}
		usable = append(usable, r)
	}
	if suspect > 0 {
		note("excluded %d session(s) with send failures from the loss/comparison analysis",
			suspect)
	}

	// 样本充足性此前只在排除之前检查过一次(顶部的 minSessions/
	// minCarriers 门槛)。排除路径能悄悄绕过它本身要防的情况——usable
	// 缩小到个位数、甚至全来自一个网络，去决定一个要维护多年的传输层。
	// 排除之后必须重新做一次同样的检查，拒绝的方式和顶部两条完全一致：
	// 明确拒绝(Conclusive=false)，理由里把排除前后的数字都摆出来，
	// 不然读的人会以为一开始就没收够。
	if len(usable) < minSessions {
		note("usable sample too small after exclusions: %d of %d sessions that passed the handshake remain usable (need at least %d)",
			len(usable), len(passed), minSessions)
		return res
	}
	usableCarriers := map[string]bool{}
	for _, r := range usable {
		usableCarriers[normaliseCarrier(r.Carrier)] = true
	}
	if len(usableCarriers) < minCarriers {
		// 排除前后的网络数都要报出来，理由和上面会话数那条一样：
		// "排除后只剩 1 个"和"一开始就只有 1 个"是完全不同的两件事——
		// 前者说明排除打偏了样本，后者说明当初就没把网络收齐。
		note("usable sample too narrow after exclusions: %d network(s) among %d usable sessions, down from %d network(s) across %d sessions (need at least %d)",
			len(usableCarriers), len(usable), len(carriers), len(rs), minCarriers)
		return res
	}

	var dgLoss []float64
	for _, r := range usable {
		dgLoss = append(dgLoss, r.Datagram.LossPercent)
	}
	if m := median(dgLoss); m > maxDatagramLoss {
		cross("median datagram loss %.1f%% (threshold %.0f%%)", m, maxDatagramLoss)
	}

	// datagram 轮被中断、但同一条连接上的 stream 轮没有被中断的会话
	// 占比——这才是"stream 回退能不能真的救回这个人"的直接问题。
	// 与它互斥地统计"两轮都被中断"：连接本身死了，不是中间设备在挑
	// datagram 特殊对待，stream 回退同样没有对象可以活在里面，所以
	// 这部分会话归入独立的 Findings，而不是 Crossed(见下面)。
	datagramOnlyInterrupt := 0
	bothInterrupted := 0
	for _, r := range usable {
		switch {
		case r.Datagram.Interrupted && !r.Stream.Interrupted:
			datagramOnlyInterrupt++
		case r.Datagram.Interrupted && r.Stream.Interrupted:
			bothInterrupted++
		}
	}
	if rate := float64(datagramOnlyInterrupt) / float64(len(usable)); rate >= maxDatagramOnlyInterruptRate {
		cross("datagram round interrupted while the stream round on the same connection was not, in %.1f%% of sessions (threshold %.0f%%)",
			rate*100, maxDatagramOnlyInterruptRate*100)
	}
	if rate := float64(bothInterrupted) / float64(len(usable)); rate >= maxBothInterruptedRate {
		finding("both rounds were interrupted on the same connection (the connection itself died, not just the datagram path) in %.1f%% of usable sessions (reporting threshold %.0f%%) — "+
			"this calls for a different-transport question, not a fallback channel inside QUIC",
			rate*100, maxBothInterruptedRate*100)
	}

	res.Conclusive = true
	res.NeedFallback = len(res.Crossed) > 0
	return res
}

func median(xs []float64) float64 {
	if len(xs) == 0 {
		return 0
	}
	s := append([]float64(nil), xs...)
	sort.Float64s(s)
	return s[len(s)/2]
}

// analyseMain 读取一个装满报告 JSON 的目录，汇总并打印判定。
// 由 main.go 的 "analyse" 子命令调用，用 args 取目录而不是直接读
// os.Args，这样它本身可以脱离命令行单独测试。
//
// 三个类别(诊断、越线理由、独立发现)分别印在自己的小节里，且 Findings
// 永远印在 VERDICT 行之后并带上"不能读成回退理由"的说明——这份输出会被
// 原样抄进最终留存的文档，握手失败率再高也不能让人一眼把它看成"需要
// stream 回退"的证据。
func analyseMain(args []string) {
	dir := "reports"
	if len(args) > 0 {
		dir = args[0]
	}
	paths, err := filepath.Glob(filepath.Join(dir, "*.json"))
	if err != nil || len(paths) == 0 {
		fmt.Fprintf(os.Stderr, "no reports found in %s\n", dir)
		os.Exit(1)
	}
	var rs []Report
	for _, p := range paths {
		b, err := os.ReadFile(p)
		if err != nil {
			fmt.Fprintf(os.Stderr, "skipping %s: %v\n", p, err)
			continue
		}
		var r Report
		if err := json.Unmarshal(b, &r); err != nil {
			fmt.Fprintf(os.Stderr, "skipping %s: %v\n", p, err)
			continue
		}
		rs = append(rs, r)
	}

	v := Verdict(rs)
	fmt.Printf("%d reports\n", len(rs))

	if len(v.Notes) > 0 {
		fmt.Println("\ndiagnostics (sample size / exclusions — informational, do not drive the verdict):")
		for _, n := range v.Notes {
			fmt.Printf("  - %s\n", n)
		}
	}

	if len(v.Crossed) > 0 {
		fmt.Println("\nthresholds crossed (these drive the fallback verdict):")
		for _, c := range v.Crossed {
			fmt.Printf("  - %s\n", c)
		}
	}

	fmt.Println()
	switch {
	case !v.Conclusive:
		fmt.Println("VERDICT: inconclusive — collect more data")
	case v.NeedFallback:
		fmt.Println("VERDICT: a stream fallback channel is required")
	default:
		fmt.Println("VERDICT: datagram-only is sufficient")
	}

	if len(v.Findings) > 0 {
		fmt.Println("\nSEPARATE FINDINGS — NOT a fallback-channel justification " +
			"(a stream fallback lives inside the same QUIC connection and cannot rescue either case below):")
		for _, f := range v.Findings {
			fmt.Printf("  - %s\n", f)
		}
	}
}
