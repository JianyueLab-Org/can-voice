// can-voice-probe analyse 汇总测试用户回传的报告并给出判定。
// 判定标准写死在下面的常量里，且在看到任何数据之前就已经定下——
// 见 P1 计划 Task 9。
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
)

const (
	minSessions          = 8
	minCarriers          = 3
	maxHandshakeFailRate = 0.02 // 2%
	maxInterruptRate     = 0.05 // 5%
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
)

// Verdict 判定是否必须实现 stream 回退通道。
//
// 三个返回值：needFallback 是判定本身；conclusive 说明这个判定是否
// 立得住（数据是否足够支撑它）；reasons 是每一条越过的阈值 + 每一条
// 诊断性说明（排除了多少可疑样本、排除后样本是否还够）。
//
// needFallback 只在 conclusive 为真时才有意义——调用方必须先看
// conclusive，见 analyseMain 的用法。之所以不用一个 bool 硬顶两件事：
// "排除了 N 个可疑样本"这类无害的说明本不该把结论拨向"必须回退"，
// 也不该被读成"结论不成立"；三个返回值把"结论是什么"和"结论立不立得住"
// 彻底分开，不需要调用方用 len(reasons)>0 去猜。
//
// 样本不足时拒绝出结论：用太少的数据决定一个要维护多年的传输层，
// 比没有数据更危险，因为它看起来像有依据——这条规矩不仅在排除之前
// 检查一次，排除之后也要重新检查一次（见下面 usable 那一段的注释），
// 否则"少于 8 个会话/少于 3 家运营商就拒绝判定"这条门槛会被排除路径
// 悄悄绕过去。
func Verdict(rs []Report) (bool, bool, []string) {
	if len(rs) < minSessions {
		return false, false, []string{fmt.Sprintf(
			"sample too small: %d sessions, need at least %d", len(rs), minSessions)}
	}
	carriers := map[string]bool{}
	for _, r := range rs {
		carriers[r.Carrier] = true
	}
	if len(carriers) < minCarriers {
		return false, false, []string{fmt.Sprintf(
			"sample too narrow: %d carriers, need at least %d", len(carriers), minCarriers)}
	}

	// reasons 里混着两种东西：真正越过了某条阈值的结论(会把 crossed
	// 置位)，和纯粹说明"这次分析排除了什么/为什么拒绝"的诊断性说明
	// (不会)。needFallback 只看 crossed，不看 reasons 是否为空——不然
	// 一条无害的"排除了 N 个可疑样本"的说明会把 need 意外地拨到 true。
	var reasons []string
	crossed := false
	cross := func(format string, args ...any) {
		reasons = append(reasons, fmt.Sprintf(format, args...))
		crossed = true
	}
	note := func(format string, args ...any) {
		reasons = append(reasons, fmt.Sprintf(format, args...))
	}

	// 握手失败的会话两轮根本没有机会开始，不该被当成"跑完了但很干净"
	// 计入下面几行；hsFail 的分母仍然是全部会话——它衡量的正是"有多少
	// 人连握手都过不去"，全部会话才是正确的分母。
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
		cross("handshake failed in %.1f%% of sessions (threshold %.0f%%)",
			rate*100, maxHandshakeFailRate*100)
	}

	if len(passed) == 0 {
		// 全部会话都没握手成功，后面几行没有对象可算——上面 hsFail 那行
		// 已经把这个情况的结论说清楚了(它必然越过了阈值：分母是全部
		// 会话，全部会话都没过握手意味着比例是 100%)，这个结论建立在
		// 全部会话(已经过了顶部的样本量/运营商门槛)之上，立得住，
		// 不用再算一次 0/0。
		return crossed, true, reasons
	}

	// datagram 轮被中断的比例：分母是"过了握手的会话数"，不是全部会话
	// 数——如果分母用全部会话数，网络越差、握手失败越多，能计入分子的
	// 会话就越少，比例反而被摊薄，原本最该触发的时候反而最难触发
	// (Ruling 3)。握手失败已经由上面单独一行覆盖，这里不重复计。
	interrupted := 0
	for _, r := range passed {
		if r.Datagram.Interrupted {
			interrupted++
		}
	}
	if rate := float64(interrupted) / float64(len(passed)); rate >= maxInterruptRate {
		cross("datagram round was interrupted in %.1f%% of sessions that passed the handshake (threshold %.0f%%)",
			rate*100, maxInterruptRate*100)
	}

	// usable 是排除了"轮次根本没跑起来"和"发送本身在失败"两类会话之后
	// 剩下的集合，下面的丢包率中位数和 datagram-vs-stream 对比都建立在
	// 它上面。
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

	// 修复轮 1：样本充足性此前只在排除之前检查过一次(顶部的 minSessions/
	// minCarriers 门槛)。排除路径能悄悄绕过它本身要防的情况——usable
	// 缩小到个位数、甚至全来自一家运营商，去决定一个要维护多年的传输层。
	// 排除之后必须重新做一次同样的检查，拒绝的方式和顶部两条完全一致：
	// 明确拒绝(conclusive=false)，理由里把排除前后的数字都摆出来，
	// 不然读的人会以为一开始就没收够。这一条吸收了原来 Ruling 6 的空集
	// 检查——len(usable)==0 只是 len(usable)<minSessions 的一个特例，
	// 不再单独判断一次。
	if len(usable) < minSessions {
		note("usable sample too small after exclusions: %d of %d sessions that passed the handshake remain usable (need at least %d)",
			len(usable), len(passed), minSessions)
		return false, false, reasons
	}
	usableCarriers := map[string]bool{}
	for _, r := range usable {
		usableCarriers[r.Carrier] = true
	}
	if len(usableCarriers) < minCarriers {
		// 排除前后的运营商数都要报出来，理由和上面会话数那条一样：
		// "排除后只剩 1 家"和"一开始就只有 1 家"是完全不同的两件事——
		// 前者说明排除打偏了样本，后者说明当初就没把网络收齐。
		note("usable sample too narrow after exclusions: %d carrier(s) among %d usable sessions, down from %d carrier(s) across %d sessions (need at least %d)",
			len(usableCarriers), len(usable), len(carriers), len(rs), minCarriers)
		return false, false, reasons
	}

	var dgLoss []float64
	for _, r := range usable {
		dgLoss = append(dgLoss, r.Datagram.LossPercent)
	}
	if m := median(dgLoss); m > maxDatagramLoss {
		cross("median datagram loss %.1f%% (threshold %.0f%%)", m, maxDatagramLoss)
	}

	// 新第四行(替换了原来的丢包率之差中位数，见上面 maxDatagramOnlyInterruptRate
	// 的注释)：datagram 轮被中断、但同一条连接上的 stream 轮没有被中断的
	// 会话占比——这才是"stream 回退能不能真的救回这个人"的直接问题。
	datagramOnlyInterrupt := 0
	for _, r := range usable {
		if r.Datagram.Interrupted && !r.Stream.Interrupted {
			datagramOnlyInterrupt++
		}
	}
	if rate := float64(datagramOnlyInterrupt) / float64(len(usable)); rate >= maxDatagramOnlyInterruptRate {
		cross("datagram round interrupted while the stream round on the same connection was not, in %.1f%% of sessions (threshold %.0f%%)",
			rate*100, maxDatagramOnlyInterruptRate*100)
	}

	return crossed, true, reasons
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

	need, conclusive, reasons := Verdict(rs)
	fmt.Printf("%d reports\n", len(rs))
	for _, r := range reasons {
		fmt.Printf("  - %s\n", r)
	}
	switch {
	case !conclusive:
		fmt.Println("VERDICT: inconclusive — collect more data")
	case need:
		fmt.Println("VERDICT: a stream fallback channel is required")
	default:
		fmt.Println("VERDICT: datagram-only is sufficient")
	}
}
