// can-voice-e2e-fixture 为 P3 的端到端测试生成全部夹具，一条命令产出：
//
//	ca.der              一张一次性的根证书（DER），Rust 侧当**额外的根证书**用
//	cert.pem / key.pem  由它签出的服务端叶证书（CN=localhost，含 127.0.0.1 的 SAN）
//	api.pub             Ed25519 公钥，裸 32 字节的 base64（服务端要的正是这个形状）
//	token.txt           一张有效的 token
//	token-expired.txt   一张已经过期的 token，用来验"拒绝要说得出原因"
//
// 刻意把证书也一起生成，而不是让人去跑 openssl：少一条要抄对的命令，
// 而且 DER 那一份 openssl 还得再转一次。
package main

import (
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/base64"
	"encoding/pem"
	"fmt"
	"math/big"
	"net"
	"os"
	"path/filepath"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/auth"
)

const outDir = "target/e2e"

func main() {
	must(os.MkdirAll(outDir, 0o755))
	writeCert()
	writeTokens()
	fmt.Printf("wrote fixtures into %s\n", outDir)
}

func writeCert() {
	// **必须是两级：一张根 + 一张由它签出的叶证书。**
	// 自签一张既 IsCA 又拿去当服务端证书用的话，webpki 会以
	// CaUsedAsEndEntity 拒掉——而症状是一条看不出所以然的
	// "invalid peer certificate"，很容易被当成"信任根没传对"。
	caKey, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	must(err)
	caTmpl := &x509.Certificate{
		SerialNumber:          serial(),
		Subject:               pkix.Name{CommonName: "can-voice e2e root"},
		NotBefore:             time.Now().Add(-time.Hour),
		NotAfter:              time.Now().Add(24 * time.Hour),
		KeyUsage:              x509.KeyUsageCertSign | x509.KeyUsageDigitalSignature,
		BasicConstraintsValid: true,
		IsCA:                  true,
	}
	caDER, err := x509.CreateCertificate(rand.Reader, caTmpl, caTmpl, &caKey.PublicKey, caKey)
	must(err)
	caCert, err := x509.ParseCertificate(caDER)
	must(err)
	must(os.WriteFile(path("ca.der"), caDER, 0o644))

	leafKey, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	must(err)
	leafTmpl := &x509.Certificate{
		SerialNumber:          serial(),
		Subject:               pkix.Name{CommonName: "localhost"},
		NotBefore:             time.Now().Add(-time.Hour),
		NotAfter:              time.Now().Add(24 * time.Hour),
		KeyUsage:              x509.KeyUsageDigitalSignature,
		ExtKeyUsage:           []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		DNSNames:              []string{"localhost"},
		IPAddresses:           []net.IP{net.ParseIP("127.0.0.1"), net.ParseIP("::1")},
		BasicConstraintsValid: true,
	}
	leafDER, err := x509.CreateCertificate(rand.Reader, leafTmpl, caCert, &leafKey.PublicKey, caKey)
	must(err)

	// 服务端送整条链（叶在前）。
	chain := append(
		pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: leafDER}),
		pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: caDER})...,
	)
	must(os.WriteFile(path("cert.pem"), chain, 0o644))

	keyDER, err := x509.MarshalPKCS8PrivateKey(leafKey)
	must(err)
	must(os.WriteFile(path("key.pem"),
		pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: keyDER}), 0o600))
}

func serial() *big.Int {
	n, err := rand.Int(rand.Reader, new(big.Int).Lsh(big.NewInt(1), 128))
	must(err)
	return n
}

func writeTokens() {
	pub, priv, err := ed25519.GenerateKey(rand.Reader)
	must(err)

	// **有效期必须落在 auth.maxTokenLifetime（10 分钟）以内。** 签一张一小时的
	// token 会被当成 ErrInvalid 拒掉——短有效期是这套设计里唯一的吊销机制，
	// 所以 exp 有上界。踩中的话，e2e 会以一条 token_invalid 失败，
	// 而那看起来像密钥不配对。
	good, err := auth.Sign(priv, auth.Claims{
		CID: "1000", Rating: 5, MaxTX: 8,
		Exp: time.Now().Add(5 * time.Minute).Unix(),
	})
	must(err)

	expired, err := auth.Sign(priv, auth.Claims{
		CID: "1000", Rating: 5, MaxTX: 8,
		Exp: time.Now().Add(-1 * time.Minute).Unix(),
	})
	must(err)

	must(os.WriteFile(path("api.pub"),
		[]byte(base64.StdEncoding.EncodeToString(pub)), 0o644))
	must(os.WriteFile(path("token.txt"), []byte(good), 0o644))
	must(os.WriteFile(path("token-expired.txt"), []byte(expired), 0o644))
}

func path(name string) string { return filepath.Join(outDir, name) }

func must(err error) {
	if err != nil {
		panic(err)
	}
}
