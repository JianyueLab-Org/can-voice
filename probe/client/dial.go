package main

import (
	"context"
	"crypto/tls"
	"time"

	"github.com/quic-go/quic-go"
)

const ALPN = "can-voice-probe/1"

// HandshakeResult 是探针的第一个测量项：QUIC 握手能不能做完。
// 握手失败是最可能的失败形态（UDP 被整体阻断），所以它的错误文本要原样留下。
type HandshakeResult struct {
	OK     bool   `json:"ok"`
	Millis int64  `json:"millis"`
	Error  string `json:"error,omitempty"`
}

// Dial 建立探针连接。无论成败都返回一个填好的 HandshakeResult。
func Dial(ctx context.Context, addr string, insecure bool) (quic.Connection, HandshakeResult, error) {
	start := time.Now()
	conn, err := quic.DialAddr(ctx, addr, &tls.Config{
		InsecureSkipVerify: insecure,
		NextProtos:         []string{ALPN},
	}, &quic.Config{
		EnableDatagrams: true,
		MaxIdleTimeout:  90 * time.Second,
		KeepAlivePeriod: 15 * time.Second,
	})
	res := HandshakeResult{Millis: time.Since(start).Milliseconds()}
	if res.Millis == 0 {
		res.Millis = 1 // 保证"尝试过"和"没尝试"可区分
	}
	if err != nil {
		res.Error = err.Error()
		return nil, res, err
	}
	res.OK = true
	return conn, res, nil
}
