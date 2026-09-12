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

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
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
}
