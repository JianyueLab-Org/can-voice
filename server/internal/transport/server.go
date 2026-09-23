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
	"sync"
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
	// MaxPendingHandshakes caps connections admitted before HELLO authentication.
	// Zero uses a conservative default.
	MaxPendingHandshakes int
	AdmissionStats       *AdmissionStats
	// Package tests exercising the pre-scope protocol may opt into old TX behavior.
	// Unexported: production callers cannot enable this compatibility bypass.
	unsafeLegacyTXForTests bool
}

// shutdownGrace 是关停时等所有连接把 CONNECTION_CLOSE 送出去的上限。
//
// 有界，因为 quic-go 的 CloseWithError 末尾是 `<-s.ctx.Done()`——它等到那条连接
// 的主循环真的退出。一条正在往一个已经不可达的对端写东西的连接可能要等上一个
// 重传周期，而关停不能被任何一个走掉的客户端拖住。
//
// 2 秒：CONNECTION_CLOSE 是一个包，本地发出去就算数；这个值留的是 quic-go 自己
// 拆循环的时间，不是等对端确认的时间（QUIC 的关闭本来就不等确认）。
//
// 是变量只为了测试能把它调小，和 handshakeTimeout 同样的理由。
var shutdownGrace = newDuration(2 * time.Second)

// Serve 起监听并接受连接，直到 ctx 取消。
func Serve(ctx context.Context, cfg Config, r *router.Router) error {
	ln, err := listen(cfg)
	if err != nil {
		return err
	}
	slog.Info("listening", "addr", ln.Addr().String(), "alpn", ALPN)
	return serve(ctx, ln, cfg, r)
}

// serve 是 Serve 去掉"建监听器"那一步之后的本体。
//
// 拆出来是为了可测：Serve 自己建监听器，所以测试没办法知道它绑到了哪个端口，
// 而关停这条路径**只能**从一个真的连上来的客户端那一侧观察。
func serve(ctx context.Context, ln *quic.Listener, cfg Config, r *router.Router) error {
	live := newConnSet()
	err := accept(ctx, ln, cfg, r, live, newAdmissionGate(cfg.MaxPendingHandshakes, cfg.AdmissionStats))

	// 顺序是这条路径的全部内容，别换。
	//
	// 原来这里是 `defer ln.Close()`，于是 accept 一返回就先关监听器。而
	// quic-go 的 Listener.Close() 走的是 Transport.closeServer()：它把 server
	// 摘掉、关掉 UDP socket，**从不遍历 handlerMap**——已经建立的连接一条都
	// 不会收到 CONNECTION_CLOSE。socket 一关，它们连发都发不出去了。
	//
	// 于是重启部署的表现是：每个客户端挂在那儿，直到自己的空闲超时（60 秒）
	// 才发现服务端没了，然后按"断网"处理。而码 0 的含义恰恰是"重启了，你可以
	// 重连"——**重启部署是唯一会产生码 0 的路径**，它送不出去就等于这个码在
	// 生产上从不存在，客户端那套"码 0 就重连、其它码别重连"的规则也就无从谈起。
	live.closeAll(shutdownGrace.Get())
	ln.Close()
	return err
}

func listen(cfg Config) (*quic.Listener, error) {
	if cfg.TLS == nil {
		// 不加这一条的话下面 Clone() 返回 nil，给 nil 写 NextProtos 直接 panic，
		// 而 panic 的堆栈指向 crypto/tls 而不是"配置里少了证书"。
		return nil, errors.New("transport: Config.TLS is nil; a QUIC listener needs a certificate")
	}
	tlsConf := cfg.TLS.Clone()
	tlsConf.NextProtos = []string{ALPN}
	return quic.ListenAddr(cfg.Addr, tlsConf, quicConfig())
}

// quicConfig 是这个监听器的 QUIC 参数。
//
// 抽成函数只为一件事：**让它能被断言**。内联在 ListenAddr 的实参里的时候，
// 这三个值一个都钉不住——把 KeepAlivePeriod 改成 0、把 MaxIdleTimeout 改成
// 一分钟以外的任何值，整套测试照绿，而它们各自都有一条能在生产上咬人的后果。
func quicConfig() *quic.Config {
	return &quic.Config{
		// 音频走不可靠 datagram。少了这一位，握手照样成功、控制面照样工作，
		// 而 SendDatagram 每一次都返回 "datagram support disabled"——
		// 所有人都在台面上亮着，谁也听不见谁。
		EnableDatagrams: true,
		// 空闲超时比任何一次正常静默都长：管制员可能几分钟不说话。
		MaxIdleTimeout: 60 * time.Second,
		// **保活不能是 0。** 零值的意思是"不发保活"，而这条连接上安静几分钟是
		// 完全正常的——一个只监听、不讲话的管制员，或者一架在巡航段没人叫的
		// 飞机。没有保活，空闲计时器在 60 秒后开火，他被断开；重连之后一切正常，
		// 于是表现是"每隔一分钟掉一次线"，而日志里只有一条普通的超时。
		// 上面那句注释（"管制员可能几分钟不说话，但 QUIC 的保活会撑住连接"）
		// 正是为这种人写的，而它论证的东西恰恰是这一行。
		//
		// 15 秒：明显小于 MaxIdleTimeout 的一半，所以丢一个 PING 也还有一次机会。
		KeepAlivePeriod: 15 * time.Second,
	}
}

// accept 循环收连接，直到 ctx 取消或者监听器出错。
//
// 返回 error 而不是 void：监听器因为 ctx 之外的原因挂掉时，
// Serve 必须把它报上去。原文 `accept(...); return ctx.Err()` 在那种情况下
// 返回 nil，于是整个进程静悄悄地"正常退出"，而端口其实已经没人在听了。
func accept(ctx context.Context, ln *quic.Listener, cfg Config, r *router.Router, live *connSet, gates ...*admissionGate) error {
	var gate *admissionGate
	if len(gates) != 0 {
		gate = gates[0]
	} else {
		gate = newAdmissionGate(cfg.MaxPendingHandshakes, cfg.AdmissionStats)
	}
	for {
		conn, err := ln.Accept(ctx)
		if err != nil {
			if ctx.Err() != nil {
				return ctx.Err()
			}
			slog.Error("accept failed", "error", err)
			return err
		}
		if !gate.tryAcquire() {
			conn.CloseWithError(CloseEvicted, "too many pending handshakes")
			continue
		}
		// 登记要在处理协程起来**之前**完成：反过来的话，一条刚接进来、还没被
		// 登记上的连接会被关停整个漏掉，而那恰好是重启那一瞬间最可能发生的事。
		// 这条顺序是 connSet.serve 的后置条件，并且在那里被钉住——写成
		// live.add(conn) 加一句 go 的话，把 add 挪进协程里整套测试照绿。
		live.serve(conn, func() { handleConn(ctx, conn, cfg, r, gate.release) })
	}
}

// connSet 是当前活着的连接，只为关停时能挨个把关闭码送出去。
//
// 不复用 router 的会话表：那里只有**握手成功**的连接，而一条正卡在握手里的
// 连接同样要被告知服务端走了；而且 router 刻意不认识 QUIC。
type connSet struct {
	mu sync.Mutex
	m  map[quic.Connection]struct{}
}

func newConnSet() *connSet {
	return &connSet{m: map[quic.Connection]struct{}{}}
}

// serve 登记一条连接，然后在一条新协程上处理它；协程退出时自动注销。
//
// **登记在本函数返回之前、并且在调用方自己的协程上完成。** 这不是风格问题，
// 是这个函数存在的全部理由。写成
//
//	live.add(conn)
//	go func() { defer live.remove(conn); handleConn(...) }()
//
// 的话，把 add 挪进协程里**整套测试照绿**（终审 L-7 实测）：窗口很窄，但它恰好在
// 重启那一瞬间最宽，而漏掉的后果是那条连接收不到关闭码 0，客户端把一次正常重启
// 当成异常掉线去查网络。两行语句的先后顺序没有任何东西钉得住；变成一个函数的
// 后置条件之后它可以被直接断言——堵住登记（拿住 mu），serve 必须卡在那里、
// fn 必须还没开始跑。见 TestAConnectionIsRegisteredBeforeItsHandlerCanStart。
func (s *connSet) serve(c quic.Connection, fn func()) {
	// 登记就地做，不留一个单独的 add 方法：留着的话，把它挪进下面那条协程里
	// 仍然是一次看着很自然的"整理"，而这个函数存在的全部意义就是那个顺序。
	s.mu.Lock()
	s.m[c] = struct{}{}
	s.mu.Unlock()

	go func() {
		defer s.remove(c)
		fn()
	}()
}

func (s *connSet) remove(c quic.Connection) {
	s.mu.Lock()
	delete(s.m, c)
	s.mu.Unlock()
}

// closeAll 给每条活着的连接发 CloseNormal，最多等 grace。
//
// 并发发而不是挨个发：CloseWithError 会等到那条连接的主循环退出，串行的话
// 一条慢连接就能把它后面所有人的关闭码拖到超时之后。
func (s *connSet) closeAll(grace time.Duration) {
	s.mu.Lock()
	conns := make([]quic.Connection, 0, len(s.m))
	for c := range s.m {
		conns = append(conns, c)
	}
	s.mu.Unlock()
	if len(conns) == 0 {
		return
	}

	slog.Info("closing live connections before shutting down",
		"connections", len(conns), "code", uint64(CloseNormal))
	done := make(chan struct{})
	go func() {
		defer close(done)
		var wg sync.WaitGroup
		wg.Add(len(conns))
		for _, c := range conns {
			go func(c quic.Connection) {
				defer wg.Done()
				// 空 reason：码 0 自己就说完了——"服务端正常关闭，你可以重连"。
				c.CloseWithError(CloseNormal, "")
			}(c)
		}
		wg.Wait()
	}()
	select {
	case <-done:
	case <-time.After(grace):
		// 没等到不是灾难：CONNECTION_CLOSE 本地发出去就算数，这里等的只是
		// quic-go 拆自己的循环。但要说一声，否则关停变慢的时候无从下手。
		slog.Warn("some connections did not finish closing within the shutdown grace",
			"connections", len(conns), "grace", grace.String())
	}
}

// SelfSignedCert 仅供测试。生产用 Let's Encrypt 证书链——
// 客户端因此不需要指纹固定，也就没有"证书换了则全网拒连"的风险（spec 6）。
// 生产证书的续期由 internal/tlscert 跟上（#68）。
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
