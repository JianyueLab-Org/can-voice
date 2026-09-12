package main

import (
	"context"
	"encoding/binary"
	"fmt"
	"sort"
	"sync"
	"time"

	"github.com/quic-go/quic-go"
)

// 与 spec §5.2 的真实语音负载对齐：13 字节头 + 约 60 字节 Opus。
const (
	payloadBytes = 73
	packetsPerS  = 50
	probeHeader  = 12
)

// Probe 是一个探测包的载荷头。回显是原样的，所以 RTT 由客户端自己算，
// 服务端不参与，也就不可能污染测量。
type Probe struct {
	Seq           uint32
	SentUnixNanos int64
}

func encodeProbe(seq uint32, now time.Time, size int) []byte {
	b := make([]byte, size)
	binary.BigEndian.PutUint32(b[0:4], seq)
	binary.BigEndian.PutUint64(b[4:12], uint64(now.UnixNano()))
	// 其余字节留零作填充，把包撑到真实语音包的大小。
	return b
}

func decodeProbe(b []byte) (Probe, error) {
	if len(b) < probeHeader {
		return Probe{}, fmt.Errorf("probe payload is %d bytes, need at least %d", len(b), probeHeader)
	}
	return Probe{
		Seq:           binary.BigEndian.Uint32(b[0:4]),
		SentUnixNanos: int64(binary.BigEndian.Uint64(b[4:12])),
	}, nil
}

// RoundResult 是一轮测量的全部结论。
type RoundResult struct {
	Sent     int `json:"sent"`
	Received int `json:"received"`
	// SendFailures 是 SendDatagram 失败的次数（与 Sent 分开计数）。
	// summarise 按 0..Sent-1 扫描序列号，假定序列号是连续的——这在
	// 健康连接上成立（seq == sent）；一旦有发送失败，seq 会比 sent
	// 跑得更快，尾窗口就不再对应真实的最后 5 秒。这种情况下非零的
	// SendFailures 本身就是比丢包率更重要的结论，所以这里不去让
	// summarise 支持稀疏序列号，只如实报告发送失败发生过。
	SendFailures      int     `json:"send_failures"`
	LossPercent       float64 `json:"loss_percent"`
	RTTMedianMs       float64 `json:"rtt_median_ms"`
	RTTP95Ms          float64 `json:"rtt_p95_ms"`
	JitterMs          float64 `json:"jitter_ms"`
	FirstLossAtSecond int     `json:"first_loss_at_second"`
	// Interrupted 表示"最后 5 秒一个回显都没有"，即中途被掐 ——
	// 与均匀丢包是完全不同的故障，需要完全不同的对策。
	Interrupted bool `json:"interrupted"`
	// Failed 表示这一轮在测量真正开始之前就已经失败（比如 stream 轮
	// 连 OpenStreamSync 都没成功）。这两个字段存在的理由：一个全零的
	// RoundResult（Sent=0, Received=0, LossPercent=0, Interrupted=false）
	// 和"跑完了、什么都没丢"的完美结果在 JSON 里长得一模一样，而两轮
	// 的结论恰恰是从两轮的差异里读出来的——一轮没跑起来却报成 0% 丢包，
	// 会把结论读反。datagram 轮目前没有对应的早期失败路径，所以它的
	// Failed 恒为 false，但字段放在共享的 RoundResult 上而不是只给
	// stream 轮加一个新类型。
	Failed bool `json:"failed"`
	// Error 携带失败原因，经过 scrubAddrs 清洗——见 dial.go 里
	// HandshakeResult.Error 的同一条注释：这个字段最终会进入测试用户
	// 手动回传的报告文件。
	Error string `json:"error,omitempty"`
}

// RunDatagramRound 以 50 包/秒发 d 时长的探测包，并行收回显，轮次结束后再等 2 秒。
func RunDatagramRound(ctx context.Context, conn quic.Connection, d time.Duration) RoundResult {
	total := int(d.Seconds()) * packetsPerS
	rtts := make(map[uint32]time.Duration, total)
	// rtts 被接收 goroutine 写、被主 goroutine 读（间接地，通过下面的快照），
	// 两者并发运行，所以必须加锁；否则是 Go 运行时会直接终止进程的
	// "fatal error: concurrent map read and map write"，在测量中途杀死探针。
	var mu sync.Mutex
	done := make(chan struct{})

	go func() {
		defer close(done)
		for {
			b, err := conn.ReceiveDatagram(ctx)
			if err != nil {
				return
			}
			p, err := decodeProbe(b)
			if err != nil {
				continue
			}
			rtt := time.Since(time.Unix(0, p.SentUnixNanos))
			mu.Lock()
			rtts[p.Seq] = rtt
			mu.Unlock()
		}
	}()

	ticker := time.NewTicker(time.Second / packetsPerS)
	defer ticker.Stop()
	sent := 0
	sendFailures := 0
	for seq := uint32(0); int(seq) < total; seq++ {
		select {
		case <-ctx.Done():
			res := summarise(sent, snapshotRTTs(&mu, rtts), packetsPerS)
			res.SendFailures = sendFailures
			return res
		case <-ticker.C:
		}
		if err := conn.SendDatagram(encodeProbe(seq, time.Now(), payloadBytes)); err != nil {
			// 发不出去本身就是结论的一部分：继续跑完，让统计反映它。
			sendFailures++
			continue
		}
		sent++
	}

	// 轮次结束后再等 2 秒，避免把在途的包算成丢失。
	drain, cancel := context.WithTimeout(ctx, 2*time.Second)
	defer cancel()
	<-drain.Done()
	// 接收 goroutine 在函数返回之后依然在跑（done 从来没被等待过）。
	// 加了锁之后这是一个内存安全的 goroutine 泄漏，在这个短命的命令行
	// 工具里是已知且可以接受的——真正让它退出需要的取消语义不在本任务
	// 范围内，不应该在这里顺手加一套没经过评审的东西。
	_ = done

	res := summarise(sent, snapshotRTTs(&mu, rtts), packetsPerS)
	res.SendFailures = sendFailures
	return res
}

// RunStreamRound 在同一条 QUIC 连接上开一条 stream，跑与 datagram 轮完全相同的流量，
// 作为 datagram 轮的对照组。同一条连接是关键：不同连接会走不同的网络路径，比较就
// 失去意义。
//
// 两轮的差异才是结论：两轮都失败 = UDP 被整体阻断，stream 回退没有意义；
// stream 通而 datagram 不通 = 中间设备在特殊对待 datagram，回退通道有用。
// stream 是可靠有序的，QUIC 自己会重传，所以 loss≈0 是这一轮的预期结果，
// 不是需要修的 bug——这一轮真正有信息量的是 RTT/抖动（队头阻塞会推高两者）
// 和 Interrupted（在 stream 上意味着连接真的断了）。
func RunStreamRound(ctx context.Context, conn quic.Connection, d time.Duration) RoundResult {
	stream, err := conn.OpenStreamSync(ctx)
	if err != nil {
		// 连 stream 都开不出来的一轮绝不能报成"跑完了、一个没丢"——
		// 见 RoundResult.Failed 上的注释。
		return RoundResult{FirstLossAtSecond: -1, Failed: true, Error: scrubAddrs(err.Error())}
	}
	defer stream.Close()

	total := int(d.Seconds()) * packetsPerS
	rtts := make(map[uint32]time.Duration, total)
	// 与 RunDatagramRound 完全相同的数据竞争：接收 goroutine 写、summarise
	// 通过快照读，两者并发运行，必须加锁——复用同一个 snapshotRTTs helper，
	// 不为 stream 轮另起一套机制。
	var mu sync.Mutex

	go func() {
		for {
			b, err := readFrame(stream)
			if err != nil {
				return
			}
			p, err := decodeProbe(b)
			if err != nil {
				continue
			}
			rtt := time.Since(time.Unix(0, p.SentUnixNanos))
			mu.Lock()
			rtts[p.Seq] = rtt
			mu.Unlock()
		}
	}()

	ticker := time.NewTicker(time.Second / packetsPerS)
	defer ticker.Stop()
	sent := 0
	sendFailures := 0
	for seq := uint32(0); int(seq) < total; seq++ {
		select {
		case <-ctx.Done():
			res := summarise(sent, snapshotRTTs(&mu, rtts), packetsPerS)
			res.SendFailures = sendFailures
			return res
		case <-ticker.C:
		}
		if err := writeFrame(stream, encodeProbe(seq, time.Now(), payloadBytes)); err != nil {
			// stream 是可靠有序的：一次写失败之后这条 stream 几乎必定已经坏了，
			// 后面的写大概率会接连失败——如实计数，而不是悄悄跑完整轮，
			// 交出一份 sent 远小于 total、却看不出原因的"统计"。
			sendFailures++
			continue
		}
		sent++
	}

	// 轮次结束后再等 2 秒，避免把在途的包算成丢失。
	drain, cancel := context.WithTimeout(ctx, 2*time.Second)
	defer cancel()
	<-drain.Done()
	// 接收 goroutine 在函数返回之后依然在跑，是已知且可以接受的 goroutine
	// 泄漏——理由与 RunDatagramRound 里的同一条注释完全一样，这里不重复
	// 实现一套没经过评审的取消语义。

	res := summarise(sent, snapshotRTTs(&mu, rtts), packetsPerS)
	res.SendFailures = sendFailures
	return res
}

// snapshotRTTs 在锁内拷贝一份 rtts，这样 summarise 本身完全不用管锁——
// 判定逻辑要能脱离并发语境单独测试，见 summarise 的注释。
func snapshotRTTs(mu *sync.Mutex, rtts map[uint32]time.Duration) map[uint32]time.Duration {
	mu.Lock()
	defer mu.Unlock()
	cp := make(map[uint32]time.Duration, len(rtts))
	for k, v := range rtts {
		cp[k] = v
	}
	return cp
}

// summarise 把原始 RTT 表折算成结论。与 RunDatagramRound 分开是为了能单独测 ——
// 网络测量的判定逻辑必须可测，否则只能靠跑真网络来验证它对不对。
// 特意保持无锁：调用方必须传入一份不会再被并发修改的快照（比如
// snapshotRTTs 拷贝出来的那种），这样判定逻辑的测试不用牵扯并发语义。
func summarise(sent int, rtts map[uint32]time.Duration, rate int) RoundResult {
	res := RoundResult{Sent: sent, Received: len(rtts), FirstLossAtSecond: -1}
	if sent > 0 {
		res.LossPercent = float64(sent-len(rtts)) / float64(sent) * 100
	}

	ms := make([]float64, 0, len(rtts))
	for _, v := range rtts {
		ms = append(ms, float64(v.Microseconds())/1000)
	}
	sort.Float64s(ms)
	if n := len(ms); n > 0 {
		res.RTTMedianMs = ms[n/2]
		res.RTTP95Ms = ms[(n*95)/100]
		res.JitterMs = ms[(n*95)/100] - ms[n/20]
	}

	for seq := 0; seq < sent; seq++ {
		if _, ok := rtts[uint32(seq)]; !ok {
			res.FirstLossAtSecond = seq / rate
			break
		}
	}

	// 最后 5 秒一个都没收到 = 被掐断，而不是均匀丢包。
	tailStart := sent - 5*rate
	if tailStart < 0 {
		tailStart = 0
	}
	res.Interrupted = true
	for seq := tailStart; seq < sent; seq++ {
		if _, ok := rtts[uint32(seq)]; ok {
			res.Interrupted = false
			break
		}
	}
	if sent == 0 {
		res.Interrupted = false
	}
	return res
}
