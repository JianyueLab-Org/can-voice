// Package transport 把 QUIC 接到 router 上。
//
// 一条 QUIC 连接承载全部：控制面走 bidirectional stream（长度前缀 JSON），
// 音频走 unreliable datagram。QUIC 的 datagram 扩展给了 UDP 的延迟特性，
// 同时免掉自写 DTLS 握手与密钥协商（spec 5）。
package transport

import (
	"context"
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"errors"
	"log/slog"
	"math/big"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/router"
	"github.com/quic-go/quic-go"
)

// ALPN 是 can-voice 的应用层协议标识。
const ALPN = "can-voice/1"

// ServerVersion 出现在 READY 里，供客户端记录与排障。
//
// 是包级常量而不是配置项：服务端的身份串不该按部署实例变化，
// 否则客户端日志里的这一行就没法用来对照版本。
const ServerVersion = "can-voice/1.0.0"

// Config 是传输层的全部配置。
type Config struct {
	Addr string
	TLS  *tls.Config
	// PublicKey 是 can-api 的 Ed25519 公钥，用来本地验签 token。
	PublicKey ed25519.PublicKey
	// MaxRX 是单个会话允许订阅的频率数上限。
	//
	// 它必须真的传给 router（见 handshake），不能只在 READY 里通告：
	// 每个 RX 频率都要在写锁里进倒排索引，一个声明了一万个频率的已鉴权会话
	// 会让全网的扇出排队等它。
	MaxRX int
}

// Serve 起监听并接受连接，直到 ctx 取消。
func Serve(ctx context.Context, cfg Config, r *router.Router) error {
	ln, err := listen(cfg)
	if err != nil {
		return err
	}
	defer ln.Close()
	slog.Info("listening", "addr", ln.Addr().String(), "alpn", ALPN)
	return accept(ctx, ln, cfg, r)
}

func listen(cfg Config) (*quic.Listener, error) {
	if cfg.TLS == nil {
		// 不加这一条的话下面 Clone() 返回 nil，给 nil 写 NextProtos 直接 panic，
		// 而 panic 的堆栈指向 crypto/tls 而不是"配置里少了证书"。
		return nil, errors.New("transport: Config.TLS is nil; a QUIC listener needs a certificate")
	}
	tlsConf := cfg.TLS.Clone()
	tlsConf.NextProtos = []string{ALPN}
	return quic.ListenAddr(cfg.Addr, tlsConf, &quic.Config{
		EnableDatagrams: true,
		// 空闲超时比任何一次正常静默都长：管制员可能几分钟不说话，
		// 但 QUIC 的保活会撑住连接。
		MaxIdleTimeout:  60 * time.Second,
		KeepAlivePeriod: 15 * time.Second,
	})
}

// accept 循环收连接，直到 ctx 取消或者监听器出错。
//
// 返回 error 而不是 void：监听器因为 ctx 之外的原因挂掉时，
// Serve 必须把它报上去。原文 `accept(...); return ctx.Err()` 在那种情况下
// 返回 nil，于是整个进程静悄悄地"正常退出"，而端口其实已经没人在听了。
func accept(ctx context.Context, ln *quic.Listener, cfg Config, r *router.Router) error {
	for {
		conn, err := ln.Accept(ctx)
		if err != nil {
			if ctx.Err() != nil {
				return ctx.Err()
			}
			slog.Error("accept failed", "error", err)
			return err
		}
		go handleConn(ctx, conn, cfg, r)
	}
}

// SelfSignedCert 仅供测试。生产用 Let's Encrypt 证书链——
// 客户端因此不需要指纹固定，也就没有"证书换了则全网拒连"的风险（spec 6）。
func SelfSignedCert() (tls.Certificate, error) {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		return tls.Certificate{}, err
	}
	tmpl := x509.Certificate{
		SerialNumber: big.NewInt(1),
		Subject:      pkix.Name{CommonName: "can-voice-test"},
		NotBefore:    time.Now().Add(-time.Hour),
		NotAfter:     time.Now().Add(24 * time.Hour),
	}
	der, err := x509.CreateCertificate(rand.Reader, &tmpl, &tmpl, &key.PublicKey, key)
	if err != nil {
		return tls.Certificate{}, err
	}
	return tls.Certificate{Certificate: [][]byte{der}, PrivateKey: key}, nil
}
