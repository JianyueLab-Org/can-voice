// Package tlscert 提供语音服务端的 TLS 证书，磁盘上的文件换了就换上新的。
//
// # 为什么不在启动时读一次就完
//
// 生产用 Let's Encrypt，证书 90 天有效，certbot 大约每 60 天在磁盘上换一张。
// 只在启动时 `tls.LoadX509KeyPair` 一次的话，进程会一直发内存里那张旧的，直到它
// 过期那天全网的客户端和通播机队同时校验失败（#68）。进程本身没有退出，
// `restart: always` 救不了它；而第一次续期在切换之后两个月左右，那时
// 起回 Murmur 的退路早就删了。
//
// 旧版没有这个问题只是因为它用的是 Murmur 自签的长期证书，客户端钉指纹。
// 换成 CA 证书链消掉了"证书一换全网拒连"，代价就是这里要自己跟上续期。
//
// # 怎么判断换了
//
// 每次握手 stat 一下两个文件（跟随软链），和上一次**成功读进来**时比：不是同一个
// 文件（certbot 把 `live/` 的软链改指到 `archive/` 的新文件）、修改时间或大小变了
// （原地覆写），就重读。一次 stat 比一次握手的密码学便宜几个数量级，不值得为它
// 另起一个轮询协程。
//
// 重读失败时**继续发旧证书**：续期换到一半（证书链接改了、私钥链接还没改）读出来
// 是对不上的一对，这一刻进来的握手不该因此失败。而且失败不记成"这个状态看过了"，
// 下一次握手照样重试——非 Docker 部署靠 deploy hook 在续期之后 chmod 私钥，
// chmod 不改修改时间，记下来的话就会一直发旧证书到过期。
package tlscert

import (
	"crypto/tls"
	"log/slog"
	"os"
	"sync"
)

// Reloader 持有当前的证书，并在文件变化时重读。并发安全。
type Reloader struct {
	certPath, keyPath string

	mu     sync.Mutex
	cert   *tls.Certificate
	loaded state // cert 是从这个状态读出来的
	// failed / failedErr 只用来压日志：同一个状态、同一个错误只记一次，
	// 不然一个坏掉的私钥会让每一次握手都打一条。
	failed    state
	failedErr string
}

// state 是两个文件在某一刻的样子。
type state struct {
	cert, key os.FileInfo
}

func (a state) same(b state) bool {
	return sameFile(a.cert, b.cert) && sameFile(a.key, b.key)
}

func sameFile(a, b os.FileInfo) bool {
	if a == nil || b == nil {
		return a == b
	}
	return os.SameFile(a, b) && a.ModTime().Equal(b.ModTime()) && a.Size() == b.Size()
}

func (r *Reloader) stat() (state, error) {
	c, err := os.Stat(r.certPath)
	if err != nil {
		return state{}, err
	}
	k, err := os.Stat(r.keyPath)
	if err != nil {
		return state{}, err
	}
	return state{cert: c, key: k}, nil
}

// Load 读进第一对证书。读不到就返回错误——启动时没有证书仍然是致命的。
func Load(certPath, keyPath string) (*Reloader, error) {
	r := &Reloader{certPath: certPath, keyPath: keyPath}
	st, err := r.stat()
	if err != nil {
		return nil, err
	}
	c, err := tls.LoadX509KeyPair(certPath, keyPath)
	if err != nil {
		return nil, err
	}
	r.cert, r.loaded = &c, st
	slog.Info("tls certificate loaded", leafAttrs(&c)...)
	return r, nil
}

// GetCertificate 给 tls.Config 用：返回当前证书，文件变了先重读。
//
// 永远不返回错误。重读失败时手里那张旧证书仍然是最好的答案：它要么还有效，
// 要么已经过期——过期的话握手本来就会失败，返回错误也不会让它成功。
func (r *Reloader) GetCertificate(*tls.ClientHelloInfo) (*tls.Certificate, error) {
	r.mu.Lock()
	defer r.mu.Unlock()

	// **先 stat 再读**，顺序不能反。反过来的话，读完、stat 之前文件换了，记下的是
	// 新状态、手里是旧内容，之后再也不会重读；这个顺序最坏只是多读一次。
	st, err := r.stat()
	if err != nil {
		r.reportFailure(state{}, err)
		return r.cert, nil
	}
	if st.same(r.loaded) {
		return r.cert, nil
	}
	c, err := tls.LoadX509KeyPair(r.certPath, r.keyPath)
	if err != nil {
		r.reportFailure(st, err)
		return r.cert, nil
	}
	r.cert, r.loaded = &c, st
	r.failed, r.failedErr = state{}, ""
	slog.Info("tls certificate reloaded", leafAttrs(&c)...)
	return r.cert, nil
}

func (r *Reloader) reportFailure(st state, err error) {
	if st.same(r.failed) && err.Error() == r.failedErr {
		return
	}
	r.failed, r.failedErr = st, err.Error()
	slog.Error("could not reload the TLS key pair; still serving the previous certificate",
		"cert", r.certPath, "key", r.keyPath, "error", err)
}

// Config 返回一份按需取证书的 tls.Config。
//
// 只填 GetCertificate，不填 Certificates：两者都有时 crypto/tls 对不带 SNI 的
// 握手会直接用 Certificates 里的第一张，换证书就又只换了一半。
func (r *Reloader) Config() *tls.Config {
	return &tls.Config{GetCertificate: r.GetCertificate}
}

// leafAttrs 是加载成功时记的那几项。到期时间最要紧：运维看日志就知道这张证书
// 还能撑多久，续期没生效也能从这一行看出来。
func leafAttrs(c *tls.Certificate) []any {
	if c.Leaf == nil {
		return nil
	}
	return []any{"subject", c.Leaf.Subject.CommonName, "not_after", c.Leaf.NotAfter}
}
