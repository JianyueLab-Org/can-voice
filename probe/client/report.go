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

	switch {
	case r.Stream.Failed && r.Datagram.Failed:
		b.WriteString("两个通道都没能跑起来，这台机器的网络对这类连接不友好。\n")
	case r.Stream.Failed:
		// 对照通道没跑起来，就没有"两轮的差异"可言——不能把 datagram
		// 轮的结果单独拿出来下结论，那正是这两轮设计要防止的"读反"。
		b.WriteString("对照通道没能跑起来，这一次测不出数据报通道是否被特殊对待，结论无法给出。\n")
	case r.Datagram.Failed:
		b.WriteString("数据报通道没能跑起来，对照通道正常——这正是我们要找的情况：这台网络很可能在针对性阻断 UDP 数据报。\n")
	case r.Datagram.Interrupted && !r.Stream.Interrupted:
		b.WriteString("数据报通道中途被掐断，对照通道没有 —— 这正是我们要找的情况。\n")
	case r.Datagram.Interrupted && r.Stream.Interrupted:
		b.WriteString("两个通道都中途被掐断，这台机器的网络对长时间连接不友好。\n")
	case r.Datagram.LossPercent > r.Stream.LossPercent+2:
		b.WriteString("数据报通道明显比对照通道差。\n")
	default:
		b.WriteString("两个通道表现接近，这台机器的网络没有特殊对待数据报。\n")
	}
	return b.String()
}
