package main

import (
	"context"
	"flag"
	"fmt"
	"log"
	"os"
	"time"
)

const probeVersion = "p1-1"

func main() {
	addr := flag.String("server", "probe.ceruleanavi.net:64739", "探针服务器地址")
	carrier := flag.String("carrier", "", "你的网络（如：中国电信 / 校园网 / 公司网络）")
	insecure := flag.Bool("insecure", false, "跳过证书校验（仅用于本地自签名测试）")
	flag.Parse()

	if *carrier == "" {
		fmt.Println("请用 -carrier 说明你用的是什么网络，例如：")
		fmt.Println("  can-voice-probe -carrier 中国电信")
		os.Exit(2)
	}

	started := time.Now()
	rep := NewReport(probeVersion, *carrier, started)

	fmt.Println("正在测试，大约需要两分半，请不要关闭窗口，也不要切换网络。")

	// 两轮各 60 秒（+ 轮内 2 秒 drain）再加上握手，5 分钟放不下两轮，
	// 放宽到 10 分钟。
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Minute)
	defer cancel()

	conn, hs, err := Dial(ctx, *addr, *insecure)
	rep.Handshake = hs
	if err != nil {
		log.Printf("handshake failed after %d ms: %v", hs.Millis, err)
	} else {
		defer conn.CloseWithError(0, "")
		log.Printf("handshake succeeded in %d ms; running the datagram round for 60 s", hs.Millis)
		dg := RunDatagramRound(ctx, conn, 60*time.Second)
		log.Printf("datagram round: sent=%d received=%d send_failures=%d loss=%.1f%% rtt_median=%.1fms interrupted=%v",
			dg.Sent, dg.Received, dg.SendFailures, dg.LossPercent, dg.RTTMedianMs, dg.Interrupted)
		rep.Datagram = dg

		log.Printf("running the stream control round for 60 s")
		st := RunStreamRound(ctx, conn, 60*time.Second)
		if st.Failed {
			// 一轮没跑起来绝不能打印成 loss=0.0%——那看起来像是"跑完了、
			// 一个没丢"，两轮的结论恰恰是从差异里读出来的，读反了就前功尽弃。
			log.Printf("stream round: FAILED to open stream: %s", st.Error)
		} else {
			log.Printf("stream round: sent=%d received=%d send_failures=%d loss=%.1f%% rtt_median=%.1fms interrupted=%v",
				st.Sent, st.Received, st.SendFailures, st.LossPercent, st.RTTMedianMs, st.Interrupted)
		}
		rep.Stream = st
	}

	fmt.Print("\n" + Summarise(rep))

	path, err := WriteReport(rep, ".")
	if err != nil {
		log.Fatalf("write report: %v", err)
	}
	fmt.Printf("\n报告已保存到：%s\n请把这个文件发回给我们。里面只有网络测量数据，没有你的任何个人信息。\n", path)
}
