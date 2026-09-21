package tlscert

import (
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/pem"
	"math/big"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/quic-go/quic-go"
)

// writePair 在 dir 下写一对 `cert<serial>.pem` / `key<serial>.pem`，返回两个路径。
// 序列号就是这一对的身份：断言"拿到的是哪张证书"只看它。
func writePair(t *testing.T, dir string, serial int64) (cert, key string) {
	t.Helper()
	k, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	tmpl := x509.Certificate{
		SerialNumber: big.NewInt(serial),
		Subject:      pkix.Name{CommonName: "can-voice-test"},
		NotBefore:    time.Now().Add(-time.Hour),
		NotAfter:     time.Now().Add(24 * time.Hour),
	}
	der, err := x509.CreateCertificate(rand.Reader, &tmpl, &tmpl, &k.PublicKey, k)
	if err != nil {
		t.Fatal(err)
	}
	keyDER, err := x509.MarshalPKCS8PrivateKey(k)
	if err != nil {
		t.Fatal(err)
	}
	cert = filepath.Join(dir, "cert"+big.NewInt(serial).String()+".pem")
	key = filepath.Join(dir, "key"+big.NewInt(serial).String()+".pem")
	if err := os.WriteFile(cert, pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(key, pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: keyDER}), 0o600); err != nil {
		t.Fatal(err)
	}
	return cert, key
}

// link 把 live/<name> 指向 target，已有的链接原子地替换掉——certbot 续期就是这么做的：
// 新证书写进 archive/ 的新文件，再把 live/ 里的软链改指过去。
func link(t *testing.T, target, name string) {
	t.Helper()
	tmp := name + ".tmp"
	if err := os.Symlink(target, tmp); err != nil {
		t.Skipf("symlinks unavailable here: %v", err)
	}
	if err := os.Rename(tmp, name); err != nil {
		t.Fatal(err)
	}
}

// letsEncrypt 搭一个 live/ + archive/ 的目录，先指向第一对证书。
func letsEncrypt(t *testing.T) (live string, archive string) {
	t.Helper()
	root := t.TempDir()
	archive = filepath.Join(root, "archive")
	live = filepath.Join(root, "live")
	for _, d := range []string{archive, live} {
		if err := os.Mkdir(d, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	cert, key := writePair(t, archive, 1)
	link(t, cert, filepath.Join(live, "fullchain.pem"))
	link(t, key, filepath.Join(live, "privkey.pem"))
	return live, archive
}

func serial(t *testing.T, r *Reloader) int64 {
	t.Helper()
	c, err := r.GetCertificate(&tls.ClientHelloInfo{})
	if err != nil {
		t.Fatalf("GetCertificate: %v", err)
	}
	leaf, err := x509.ParseCertificate(c.Certificate[0])
	if err != nil {
		t.Fatal(err)
	}
	return leaf.SerialNumber.Int64()
}

func TestLoadFailsWhenThePairIsUnreadable(t *testing.T) {
	dir := t.TempDir()
	// 启动时读不到证书仍然是致命的：一个没有证书的语音服务端没有任何可服务的东西。
	if _, err := Load(filepath.Join(dir, "missing.pem"), filepath.Join(dir, "missing-key.pem")); err == nil {
		t.Fatal("Load must fail when the files do not exist")
	}
}

func TestServesTheInitialPair(t *testing.T) {
	live, _ := letsEncrypt(t)
	r, err := Load(filepath.Join(live, "fullchain.pem"), filepath.Join(live, "privkey.pem"))
	if err != nil {
		t.Fatal(err)
	}
	if got := serial(t, r); got != 1 {
		t.Fatalf("serial = %d, want 1", got)
	}
}

// LoadX509KeyPair 不填 Leaf。不自己 parse 的话 leafAttrs 什么都不记，
// 续期有没有接上从日志看不出来——那正是这条日志存在的理由。
func TestTheLoadedCertificateExposesTheLeaf(t *testing.T) {
	live, _ := letsEncrypt(t)
	r, err := Load(filepath.Join(live, "fullchain.pem"), filepath.Join(live, "privkey.pem"))
	if err != nil {
		t.Fatal(err)
	}
	c, err := r.GetCertificate(&tls.ClientHelloInfo{})
	if err != nil {
		t.Fatal(err)
	}
	if c.Leaf == nil {
		t.Fatal("Leaf must be parsed so the load log can print not_after")
	}
	if c.Leaf.Subject.CommonName != "can-voice-test" {
		t.Fatalf("subject = %q, want can-voice-test", c.Leaf.Subject.CommonName)
	}
	if c.Leaf.NotAfter.IsZero() {
		t.Fatal("not_after must be set")
	}
}

// 这就是 #68：certbot 续期只换磁盘上的文件，进程不重启。
func TestPicksUpARenewalWithoutRestarting(t *testing.T) {
	live, archive := letsEncrypt(t)
	r, err := Load(filepath.Join(live, "fullchain.pem"), filepath.Join(live, "privkey.pem"))
	if err != nil {
		t.Fatal(err)
	}
	serial(t, r)

	cert, key := writePair(t, archive, 2)
	link(t, cert, filepath.Join(live, "fullchain.pem"))
	link(t, key, filepath.Join(live, "privkey.pem"))

	if got := serial(t, r); got != 2 {
		t.Fatalf("after renewal serial = %d, want 2", got)
	}
}

// 不经软链、原地覆写同一个文件也要认得出来——不用 certbot、手动拷证书的部署就是这样。
func TestPicksUpAFileRewrittenInPlace(t *testing.T) {
	dir := t.TempDir()
	cert1, key1 := writePair(t, dir, 1)
	r, err := Load(cert1, key1)
	if err != nil {
		t.Fatal(err)
	}
	serial(t, r)

	cert2, key2 := writePair(t, dir, 2)
	for src, dst := range map[string]string{cert2: cert1, key2: key1} {
		b, err := os.ReadFile(src)
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(dst, b, 0o600); err != nil {
			t.Fatal(err)
		}
		// 真实的覆写发生在加载之后几十天，修改时间必然不同；测试里两次写可能落在
		// 同一个时钟刻度上，所以显式往后拨，免得这条测试靠运气。
		later := time.Now().Add(time.Minute)
		if err := os.Chtimes(dst, later, later); err != nil {
			t.Fatal(err)
		}
	}

	if got := serial(t, r); got != 2 {
		t.Fatalf("after rewrite serial = %d, want 2", got)
	}
}

// 续期换到一半（证书链接已改、私钥链接还没改）读出来的是一对对不上的文件。
// 这时必须继续发旧证书，而不是让这一刻进来的握手失败；两边都换完之后再换上新的。
func TestKeepsServingTheOldPairWhileTheNewOneIsInconsistent(t *testing.T) {
	live, archive := letsEncrypt(t)
	r, err := Load(filepath.Join(live, "fullchain.pem"), filepath.Join(live, "privkey.pem"))
	if err != nil {
		t.Fatal(err)
	}
	serial(t, r)

	cert, key := writePair(t, archive, 2)
	link(t, cert, filepath.Join(live, "fullchain.pem"))
	if got := serial(t, r); got != 1 {
		t.Fatalf("with a mismatched pair serial = %d, want the old 1", got)
	}

	link(t, key, filepath.Join(live, "privkey.pem"))
	if got := serial(t, r); got != 2 {
		t.Fatalf("once both halves moved serial = %d, want 2", got)
	}
}

// 非 Docker 部署要靠 certbot 的 deploy hook 在续期**之后**把新私钥 chmod 给服务账号
// （server/README.md）。链接改指和 chmod 之间进来的握手会读失败，而 chmod 不改修改时间
// ——所以一次失败的重读不能被记成"这个状态已经看过了"，否则就一直发旧证书直到过期。
func TestRetriesAFailedReloadEvenIfTheFilesDoNotChangeAgain(t *testing.T) {
	if os.Geteuid() == 0 {
		t.Skip("root reads a 0000 file; the permission failure cannot be reproduced")
	}
	live, archive := letsEncrypt(t)
	r, err := Load(filepath.Join(live, "fullchain.pem"), filepath.Join(live, "privkey.pem"))
	if err != nil {
		t.Fatal(err)
	}
	serial(t, r)

	cert, key := writePair(t, archive, 2)
	if err := os.Chmod(key, 0o000); err != nil {
		t.Fatal(err)
	}
	link(t, cert, filepath.Join(live, "fullchain.pem"))
	link(t, key, filepath.Join(live, "privkey.pem"))
	if got := serial(t, r); got != 1 {
		t.Fatalf("with an unreadable key serial = %d, want the old 1", got)
	}

	if err := os.Chmod(key, 0o600); err != nil {
		t.Fatal(err)
	}
	if got := serial(t, r); got != 2 {
		t.Fatalf("after the key became readable serial = %d, want 2", got)
	}
}

// 真握手，不是只调 GetCertificate；而且走 QUIC，不是 crypto/tls 直连——生产上取证书的
// 是 quic-go，它包了一层 tls.Config 再交给 crypto/tls。`Config` 返回的如果还是静态的
// `Certificates`，或者 quic-go 那一层不理 GetCertificate，上面几条照样全绿，而 #68 原样还在。
func TestQUICHandshakeGetsTheRenewedCertificate(t *testing.T) {
	live, archive := letsEncrypt(t)
	r, err := Load(filepath.Join(live, "fullchain.pem"), filepath.Join(live, "privkey.pem"))
	if err != nil {
		t.Fatal(err)
	}
	conf := r.Config()
	conf.NextProtos = []string{"can-voice-test"}
	ln, err := quic.ListenAddr("127.0.0.1:0", conf, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	go func() {
		for {
			c, err := ln.Accept(context.Background())
			if err != nil {
				return
			}
			c.CloseWithError(0, "")
		}
	}()

	handshake := func() int64 {
		t.Helper()
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		c, err := quic.DialAddr(ctx, ln.Addr().String(), &tls.Config{
			InsecureSkipVerify: true,
			NextProtos:         []string{"can-voice-test"},
		}, nil)
		if err != nil {
			t.Fatalf("dial: %v", err)
		}
		defer c.CloseWithError(0, "")
		return c.ConnectionState().TLS.PeerCertificates[0].SerialNumber.Int64()
	}

	if got := handshake(); got != 1 {
		t.Fatalf("serial = %d, want 1", got)
	}
	cert, key := writePair(t, archive, 2)
	link(t, cert, filepath.Join(live, "fullchain.pem"))
	link(t, key, filepath.Join(live, "privkey.pem"))
	if got := handshake(); got != 2 {
		t.Fatalf("after renewal serial = %d, want 2", got)
	}
}
