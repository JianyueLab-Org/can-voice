package auth

import (
	"crypto/ed25519"
	"crypto/rand"
	"errors"
	"strings"
	"testing"
	"time"
)

func keys(t *testing.T) (ed25519.PublicKey, ed25519.PrivateKey) {
	t.Helper()
	pub, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatalf("GenerateKey: %v", err)
	}
	return pub, priv
}

func TestVerifyAcceptsAFreshToken(t *testing.T) {
	pub, priv := keys(t)
	now := time.Unix(1757000000, 0)
	in := Claims{CID: "1000", Rating: 5, MaxTX: 8, Exp: now.Add(60 * time.Second).Unix()}
	tok, err := Sign(priv, in)
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	got, err := Verify(pub, tok, now)
	if err != nil {
		t.Fatalf("Verify: %v", err)
	}
	if got != in {
		t.Fatalf("claims = %+v, want %+v", got, in)
	}
}

func TestVerifyRejectsAnExpiredToken(t *testing.T) {
	pub, priv := keys(t)
	now := time.Unix(1757000000, 0)
	tok, _ := Sign(priv, Claims{CID: "1000", Exp: now.Add(-time.Second).Unix()})
	_, err := Verify(pub, tok, now)
	if err == nil {
		t.Fatal("Verify must reject an expired token")
	}
	// 哨兵必须接对：传输层拿它来决定发 "token_expired" 还是 "token_invalid"，
	// 而那是客户端唯一能用来区分"去换一张 token 再试"和"别试了"的信号。
	if !errors.Is(err, ErrExpired) {
		t.Fatalf("err = %v, want errors.Is(err, ErrExpired)", err)
	}
	if errors.Is(err, ErrInvalid) {
		t.Fatalf("err = %v is also ErrInvalid; the two must not overlap or reasonFor's switch becomes order-dependent", err)
	}
}

// exp 恰好等于 now 视为已过期（右开区间：token 在 [iat, exp) 内有效）。
// 这是刻意选择而非疏漏——见 token.go 里 Verify 的注释。
func TestVerifyRejectsATokenExpiringExactlyNow(t *testing.T) {
	pub, priv := keys(t)
	now := time.Unix(1757000000, 0)
	tok, _ := Sign(priv, Claims{CID: "1000", Exp: now.Unix()})
	_, err := Verify(pub, tok, now)
	if err == nil {
		t.Fatal("Verify must reject a token whose exp equals now")
	}
	if !errors.Is(err, ErrExpired) {
		t.Fatalf("err = %v, want errors.Is(err, ErrExpired)", err)
	}
}

func TestVerifyRejectsATamperedPayload(t *testing.T) {
	pub, priv := keys(t)
	now := time.Unix(1757000000, 0)
	tok, _ := Sign(priv, Claims{CID: "1000", Rating: 1, Exp: now.Add(60 * time.Second).Unix()})

	// 改一个字符，签名必须失配。
	parts := strings.SplitN(tok, ".", 2)
	tampered := parts[0][:len(parts[0])-1] + "A" + "." + parts[1]
	if tampered == tok {
		t.Fatal("test did not actually change the payload")
	}
	_, err := Verify(pub, tampered, now)
	if err == nil {
		t.Fatal("Verify must reject a tampered payload")
	}
	if !errors.Is(err, ErrInvalid) {
		t.Fatalf("err = %v, want errors.Is(err, ErrInvalid)", err)
	}
}

func TestVerifyRejectsATokenSignedByAnotherKey(t *testing.T) {
	pub, _ := keys(t)
	_, otherPriv := keys(t)
	now := time.Unix(1757000000, 0)
	tok, _ := Sign(otherPriv, Claims{CID: "1000", Exp: now.Add(60 * time.Second).Unix()})
	_, err := Verify(pub, tok, now)
	if err == nil {
		t.Fatal("Verify must reject a token signed by a key it does not trust")
	}
	if !errors.Is(err, ErrInvalid) {
		t.Fatalf("err = %v, want errors.Is(err, ErrInvalid)", err)
	}
}

func TestVerifyRejectsMalformedTokens(t *testing.T) {
	pub, _ := keys(t)
	now := time.Unix(1757000000, 0)
	for _, tok := range []string{"", "nodot", "a.b.c", ".", "!!!.###"} {
		_, err := Verify(pub, tok, now)
		if err == nil {
			t.Fatalf("Verify must reject malformed token %q", tok)
		}
		if !errors.Is(err, ErrInvalid) {
			t.Fatalf("malformed token %q gave %v, want errors.Is(err, ErrInvalid)", tok, err)
		}
	}
}

// 显式覆盖 brief 里点名的几种畸形输入：签名截断、签名长度错误、payload 非法 base64,
// 以确保 ed25519.Verify 永远拿到定长的公钥/签名，不会 panic。
func TestVerifyRejectsTruncatedAndWrongLengthSignatures(t *testing.T) {
	pub, priv := keys(t)
	now := time.Unix(1757000000, 0)
	tok, err := Sign(priv, Claims{CID: "1000", Exp: now.Add(60 * time.Second).Unix()})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	body, sig, ok := strings.Cut(tok, ".")
	if !ok {
		t.Fatalf("token has no separator: %q", tok)
	}

	cases := map[string]string{
		"truncated signature":         body + "." + sig[:len(sig)-4],
		"empty signature":             body + ".",
		"one-byte signature":          body + "." + enc.EncodeToString([]byte{0x01}),
		"oversized signature":         body + "." + enc.EncodeToString(make([]byte, 128)),
		"non-base64 signature":        body + ".!!!not-base64!!!",
		"non-base64 payload":          "!!!not-base64!!!." + sig,
		"payload decodes to non-json": enc.EncodeToString([]byte("not json")) + "." + sig,
	}
	for name, tampered := range cases {
		t.Run(name, func(t *testing.T) {
			_, err := Verify(pub, tampered, now)
			if err == nil {
				t.Fatalf("Verify must reject %s", name)
			}
			if !errors.Is(err, ErrInvalid) {
				t.Fatalf("%s gave %v, want errors.Is(err, ErrInvalid)", name, err)
			}
		})
	}
}

func TestVerifyDoesNotPanicOnAnyInput(t *testing.T) {
	pub, _ := keys(t)
	now := time.Unix(1757000000, 0)
	inputs := []string{
		"", ".", "..", "a", "a.", ".b", "a.b.c", "a.b.c.d",
		strings.Repeat("A", 10000) + "." + strings.Repeat("B", 10000),
		"\x00\x01\x02.\x03\x04\x05",
	}
	for _, in := range inputs {
		func() {
			defer func() {
				if r := recover(); r != nil {
					t.Fatalf("Verify panicked on input %q: %v", in, r)
				}
			}()
			Verify(pub, in, now)
		}()
	}
}

// TestAWrongLengthPublicKeyIsNotBlamedOnTheToken 钉住哨兵的边界。
//
// 公钥长度不对说的是**服务端配错了**，不是对端的 token 有问题。包上 ErrInvalid
// 的话，一次配置事故会让每一个正当客户端收到 "token_invalid"，于是所有人都被
// 送去重新取 token——而新 token 一样验不过。这条必须落到 reasonFor 的 default 分支。
func TestAWrongLengthPublicKeyIsNotBlamedOnTheToken(t *testing.T) {
	_, priv := keys(t)
	now := time.Unix(1757000000, 0)
	tok, err := Sign(priv, Claims{CID: "1000", Exp: now.Add(60 * time.Second).Unix()})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	_, err = Verify(ed25519.PublicKey{1, 2, 3}, tok, now)
	if err == nil {
		t.Fatal("Verify must reject a public key of the wrong length")
	}
	if errors.Is(err, ErrInvalid) || errors.Is(err, ErrExpired) {
		t.Fatalf("err = %v carries a token sentinel; a server misconfiguration must not be reported to the peer as a token problem", err)
	}
}

// TestTheDetailedReasonSurvivesTheWrapping 钉住"粗粒度只对外，日志仍然拿得到细节"。
//
// 哨兵是给客户端看的两级代码；服务端排障要的是原来那句话。包装如果把它吃掉，
// 日志里就只剩 "token invalid"，而那等于没说。
func TestTheDetailedReasonSurvivesTheWrapping(t *testing.T) {
	pub, priv := keys(t)
	now := time.Unix(1757000000, 0)
	tok, _ := Sign(priv, Claims{CID: "1000", Exp: now.Add(-time.Minute).Unix()})
	_, err := Verify(pub, tok, now)
	if err == nil {
		t.Fatal("Verify must reject an expired token")
	}
	if !strings.Contains(err.Error(), "1757") {
		t.Fatalf("err = %q no longer names the clock; the detail is what the server log needs", err)
	}
}
