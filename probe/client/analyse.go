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
// 样本不足时拒绝出结论：用太少的数据决定一个要维护多年的传输层，
// 比没有数据更危险，因为它看起来像有依据。
func Verdict(rs []Report) (bool, []string) {
	if len(rs) < minSessions {
		return false, []string{fmt.Sprintf(
			"sample too small: %d sessions, need at least %d", len(rs), minSessions)}
	}
	carriers := map[string]bool{}
	for _, r := range rs {
		carriers[r.Carrier] = true
	}
	if len(carriers) < minCarriers {
		return false, []string{fmt.Sprintf(
			"sample too narrow: %d carriers, need at least %d", len(carriers), minCarriers)}
	}

	// reasons 里混着两种东西：真正越过了某条阈值的结论(会把 crossed
	// 置位)，和纯粹说明"这次分析排除了什么"的诊断性说明(不会)。
	// needFallback 只看 crossed，不看 reasons 是否为空——不然一条无害的
	// "排除了 N 个可疑样本"的说明会把 need 意外地拨到 true。
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
		// 已经把这个情况的结论说清楚了(如果它越过了阈值)，这里不用再算
		// 一次 0/0。
		return crossed, reasons
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

	if len(usable) == 0 {
		// 排除之后没有能用的样本了。median(nil) 会返回 0，而 0 天然读作
		// "低于阈值"——如果这里不专门挡住，一批"stream 轮根本没跑起来"
		// 的会话会被悄悄判定成"不需要回退"。必须像样本太少、运营商太少
		// 那样明确拒绝下结论 (Ruling 6)，而不是让空切片的默认值蒙混过关。
		note("no usable sessions for the loss/comparison analysis after excluding rounds that never ran and sessions with send failures (%d sessions passed the handshake)",
			len(passed))
		return crossed, reasons
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

	return crossed, reasons
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

	need, reasons := Verdict(rs)
	fmt.Printf("%d reports\n", len(rs))
	for _, r := range reasons {
		fmt.Printf("  - %s\n", r)
	}
	if need {
		fmt.Println("VERDICT: a stream fallback channel is required")
	} else if len(reasons) > 0 {
		fmt.Println("VERDICT: inconclusive — collect more data")
	} else {
		fmt.Println("VERDICT: datagram-only is sufficient")
	}
}
