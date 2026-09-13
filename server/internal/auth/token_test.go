package auth

import (
	"crypto/ed25519"
	"crypto/rand"
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
	if _, err := Verify(pub, tok, now); err == nil {
		t.Fatal("Verify must reject an expired token")
	}
}

// exp 恰好等于 now 视为已过期（右开区间：token 在 [iat, exp) 内有效）。
// 这是刻意选择而非疏漏——见 token.go 里 Verify 的注释。
func TestVerifyRejectsATokenExpiringExactlyNow(t *testing.T) {
	pub, priv := keys(t)
	now := time.Unix(1757000000, 0)
	tok, _ := Sign(priv, Claims{CID: "1000", Exp: now.Unix()})
	if _, err := Verify(pub, tok, now); err == nil {
		t.Fatal("Verify must reject a token whose exp equals now")
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
	if _, err := Verify(pub, tampered, now); err == nil {
		t.Fatal("Verify must reject a tampered payload")
	}
}

func TestVerifyRejectsATokenSignedByAnotherKey(t *testing.T) {
	pub, _ := keys(t)
	_, otherPriv := keys(t)
	now := time.Unix(1757000000, 0)
	tok, _ := Sign(otherPriv, Claims{CID: "1000", Exp: now.Add(60 * time.Second).Unix()})
	if _, err := Verify(pub, tok, now); err == nil {
		t.Fatal("Verify must reject a token signed by a key it does not trust")
	}
}

func TestVerifyRejectsMalformedTokens(t *testing.T) {
	pub, _ := keys(t)
	now := time.Unix(1757000000, 0)
	for _, tok := range []string{"", "nodot", "a.b.c", ".", "!!!.###"} {
		if _, err := Verify(pub, tok, now); err == nil {
			t.Fatalf("Verify must reject malformed token %q", tok)
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
			if _, err := Verify(pub, tampered, now); err == nil {
				t.Fatalf("Verify must reject %s", name)
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
