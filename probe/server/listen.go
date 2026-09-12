// Package main 是 P1 连通性探针的服务端。丢弃型代码，结论产出后删除。
package main

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"math/big"
	"time"

	"github.com/quic-go/quic-go"
)

// ALPN 和生产用的 can-voice/1 刻意不同：探针与真正的语音服务可能同时在线，
// 用不同的 ALPN 保证两者不会互相误连。
const ALPN = "can-voice-probe/1"

// Listen 起一个只接受探针 ALPN 的 QUIC 监听器。
// tlsConf 会被原地改写 NextProtos —— 探针不接受调用方自带的协议列表，
// 否则一个拼错的 ALPN 会表现为“所有连接都超时”，而那正是我们要测量的信号。
func Listen(addr string, tlsConf *tls.Config) (*quic.Listener, error) {
	tlsConf.NextProtos = []string{ALPN}
	return quic.ListenAddr(addr, tlsConf, &quic.Config{
		EnableDatagrams: true,
		// 探针要能分辨“被中间设备掐掉”和“自己超时了”，所以空闲超时设得比
		// 测量轮时长（60s）长一截，让前者成为唯一可能的中断原因。
		MaxIdleTimeout:  90 * time.Second,
		KeepAlivePeriod: 15 * time.Second,
	})
}

// SelfSignedCert 仅供本地测试使用。公网部署用 Let's Encrypt，见 Task 8。
func SelfSignedCert() (tls.Certificate, error) {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		return tls.Certificate{}, err
	}
	tmpl := x509.Certificate{
		SerialNumber: big.NewInt(1),
		Subject:      pkix.Name{CommonName: "can-voice-probe"},
		NotBefore:    time.Now().Add(-time.Hour),
		NotAfter:     time.Now().Add(24 * time.Hour),
	}
	der, err := x509.CreateCertificate(rand.Reader, &tmpl, &tmpl, &key.PublicKey, key)
	if err != nil {
		return tls.Certificate{}, err
	}
	return tls.Certificate{Certificate: [][]byte{der}, PrivateKey: key}, nil
}

// Serve 在 Task 2 填实。
func Serve(ln *quic.Listener) { select {} }
