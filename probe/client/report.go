package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"time"
)

// Report 是用户回传给我们的全部内容。
// 刻意不含 IP、主机名、用户名 —— 运营商由用户手填，其余全是网络测量值。
type Report struct {
	SchemaVersion int             `json:"schema_version"`
	ProbeVersion  string          `json:"probe_version"`
	OS            string          `json:"os"`
	Arch          string          `json:"arch"`
	Carrier       string          `json:"carrier"`
	// Timezone 是缩写（如 CST），不能单独使用——它同时是"中国标准时间"和
	// "美国中部时间"的缩写，光凭它分不清用户在哪个时区。真正能用来对齐当地
	// 时刻（从而分辨"晚高峰拥塞"和"被封锁"）的是 StartedAt 里带偏移量的
	// 本地时间，Timezone 只是给人看的补充标签。
	Timezone  string          `json:"timezone"`
	StartedAt string          `json:"started_at"`
	Handshake HandshakeResult `json:"handshake"`
	Datagram  RoundResult     `json:"datagram"`
	Stream    RoundResult     `json:"stream"`
}

// NewReport 填好与网络无关的字段。
func NewReport(version, carrier string, startedAt time.Time) Report {
	tz, _ := startedAt.Zone()
	return Report{
		SchemaVersion: 1,
		ProbeVersion:  version,
		OS:            runtime.GOOS,
		Arch:          runtime.GOARCH,
		Carrier:       carrier,
		Timezone:      tz,
		// 本地时间的 RFC3339（带 +08:00 这样的偏移量），而不是 UTC——
		// 这样一个值里同时带着"什么时刻"和"哪个时区"，才能把结果和
		// 当地的时间对上号。
		StartedAt: startedAt.Format(time.RFC3339),
	}
}

// WriteReport 把报告写成带时间戳的 JSON 文件，返回路径。
// 缩进过 —— 用户被要求先读一遍再发回来。
func WriteReport(r Report, dir string) (string, error) {
	b, err := json.MarshalIndent(r, "", "  ")
	if err != nil {
		return "", err
	}
	name := fmt.Sprintf("can-voice-probe-%s.json", time.Now().Format("20060102-150405"))
	path := filepath.Join(dir, name)
	if err := os.WriteFile(path, append(b, '\n'), 0o644); err != nil {
		return "", err
	}
	return path, nil
}

// unranRound 描述"这一轮根本没有机会开始"——不是跑起来又失败，也不是留着
// 零值不管。一个全零的 RoundResult（Sent=0, Received=0, LossPercent=0,
// Failed=false）在 JSON 里跟"跑完了、一个没丢"长得一模一样：握手失败时
// 如果不显式标注，测试用户手动回传的文件、以及 Task 9 汇总工具要解析的
// 输入，都会把这种零值读成"网络很好"。形状特意和 round.go 里
// RunStreamRound 对 OpenStreamSync 失败的早期返回一致
// （FirstLossAtSecond: -1, Failed: true），这样 Task 9 排除 Failed 轮次时
// 不用为握手失败单开一条特例。
func unranRound(reason string) RoundResult {
	return RoundResult{
		FirstLossAtSecond: -1,
		Failed:            true,
		Error:             reason,
	}
}

// markRoundsAsNotRun 在握手失败后把两轮都标成"没有运行"。握手都没成功，
// 两轮都没有机会开始，所以两个字段一起置位，不去猜"也许某一轮本可以跑
// 起来"——两轮共用同一条连接，握手是两轮共同的前提。
func markRoundsAsNotRun(r *Report) {
	const reason = "handshake failed; round never started"
	r.Datagram = unranRound(reason)
	r.Stream = unranRound(reason)
}

// summariseRound 把一轮的结果写成人话，供 Summarise 对两轮分别调用。
// 单独抽出来是因为"这一轮到底跑没跑起来、丢没丢全"这几条判断本身就值得
// 复用和单独看清楚，不想把它们都摊平写在 Summarise 一个函数里。
func summariseRound(label string, r RoundResult) string {
	var b strings.Builder
	if r.Failed {
		// 这一轮在测量真正开始前就已经失败（比如 stream 轮连
		// OpenStreamSync 都没成功）。绝不能落到下面按 Sent/Received
		// 画出的百分比——那两个字段都是零，看起来会像"跑完了、一个都
		// 没丢"，而事实是这一轮根本没有产生任何测量数据。
		fmt.Fprintf(&b, "%s：没有跑起来（%s）\n", label, r.Error)
		return b.String()
	}
	fmt.Fprintf(&b, "%s：发出 %d 个，收到 %d 个，", label, r.Sent, r.Received)
	if r.Sent > 0 && r.Received == 0 {
		// 一个都没收到是最需要说清楚的形状：LossPercent 是否被调用方算过
		// 是个实现细节，不该靠它来判断"全部丢失"——直接看 Sent/Received
		// 本身。
		b.WriteString("全部丢失（100%）")
	} else {
		fmt.Fprintf(&b, "丢失 %.1f%%", r.LossPercent)
	}
	fmt.Fprintf(&b, "，延迟中位数 %.0f 毫秒", r.RTTMedianMs)
	if r.SendFailures > 0 {
		// 发送本身在失败，这一轮里的每个数字都值得怀疑，必须让用户看到。
		fmt.Fprintf(&b, "（发送失败 %d 次，这一轮的数字仅供参考）", r.SendFailures)
	}
	b.WriteString("\n")
	return b.String()
}

// Summarise 给用户看的中文摘要。最重要的一句是两轮的对比 ——
// 那才是决定要不要做 stream 回退通道的依据。
func Summarise(r Report) string {
	var b strings.Builder
	if !r.Handshake.OK {
		fmt.Fprintf(&b, "连接失败（%d 毫秒后放弃）：%s\n", r.Handshake.Millis, r.Handshake.Error)
		b.WriteString("这台机器的网络很可能整体阻断了 UDP。\n")
		return b.String()
	}
	fmt.Fprintf(&b, "连接成功，握手耗时 %d 毫秒。\n\n", r.Handshake.Millis)
	b.WriteString(summariseRound("数据报通道", r.Datagram))
	b.WriteString(summariseRound("对照通道  ", r.Stream))
	b.WriteString("\n")

	// 走到这里说明 r.Handshake.OK 为真(上面已经对握手失败提前返回过)。
	// Datagram.Failed 只可能由 markRoundsAsNotRun 置位，而那个函数只在
	// 握手失败时被调用——round.go 里也写了 datagram 轮目前没有对应的
	// 早期失败路径，它的 Failed 恒为 false。所以下面不会出现
	// r.Datagram.Failed == true 的情况：曾经写过的"两个通道都没跑起来"、
	// "数据报没跑起来但对照通道中途被掐断"、"数据报没跑起来"三个分支
	// 永远进不去，是死代码，删掉——不留着，免得以后被人当成还有用而
	// 重新踩进这个坑（终审 Finding D）。
	switch {
	case r.Stream.Failed:
		// 对照通道没跑起来，就没有"两轮的差异"可言——不能把 datagram
		// 轮的结果单独拿出来下结论，那正是这两轮设计要防止的"读反"。
		b.WriteString("对照通道没能跑起来，这一次测不出数据报通道是否被特殊对待，结论无法给出。\n")
	case r.Datagram.Interrupted && !r.Stream.Interrupted:
		b.WriteString("数据报通道中途被掐断，对照通道没有 —— 这正是我们要找的情况。\n")
	case r.Datagram.Interrupted && r.Stream.Interrupted:
		b.WriteString("两个通道都中途被掐断，这台机器的网络对长时间连接不友好。\n")
	case r.Datagram.LossPercent > maxDatagramLoss:
		// 终审 Finding E：这里原来比较的是"数据报丢包率 - 对照通道丢包率
		// > 2"，跟汇总判定表里已经删掉的旧指标是同一个算式(stream 轮
		// 丢包率恒为 ~0，两者算术上等价)，而且这个 2 是凭空写的，跟
		// analyse.go 里真正用来下结论的 maxDatagramLoss 只是数值凑巧
		// 相同、含义并不是一回事。改成直接对照汇总判定真正采信的量
		// (datagram 轮丢包率本身)和它的阈值，不再提"和对照通道的差值"，
		// 并且明说这只是单份报告的直觉提示，真正的结论要看多份报告汇总
		// 后的中位数——不能让人拿单独一份报告里的这句话当独立证据引用。
		fmt.Fprintf(&b, "数据报通道丢包率 %.1f%%，高于我们汇总判定时使用的 %.0f%% 参考线（最终结论以多份报告汇总后的中位数为准，这一份单独看不能说明什么）。\n",
			r.Datagram.LossPercent, maxDatagramLoss)
	default:
		b.WriteString("两个通道表现接近，这台机器的网络没有特殊对待数据报。\n")
	}
	return b.String()
}
