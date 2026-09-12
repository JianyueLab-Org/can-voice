package main

import (
	"context"
	"flag"
	"log"
	"time"
)

func main() {
	addr := flag.String("server", "probe.ceruleanavi.net:64739", "探针服务器地址")
	insecure := flag.Bool("insecure", false, "跳过证书校验（仅用于本地自签名测试）")
	flag.Parse()

	// 两轮各 60 秒（+ 轮内 2 秒 drain）再加上握手，5 分钟放不下两轮，
	// 放宽到 10 分钟。
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Minute)
	defer cancel()

	conn, hs, err := Dial(ctx, *addr, *insecure)
	if err != nil {
		log.Printf("handshake failed after %d ms: %v", hs.Millis, err)
		return
	}
	defer conn.CloseWithError(0, "")
	log.Printf("handshake succeeded in %d ms; running the datagram round for 60 s", hs.Millis)
	dg := RunDatagramRound(ctx, conn, 60*time.Second)
	log.Printf("datagram round: sent=%d received=%d send_failures=%d loss=%.1f%% rtt_median=%.1fms interrupted=%v",
		dg.Sent, dg.Received, dg.SendFailures, dg.LossPercent, dg.RTTMedianMs, dg.Interrupted)

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
}
