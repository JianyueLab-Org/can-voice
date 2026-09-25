package auth

import (
	"crypto/ed25519"
	"crypto/rand"
	"errors"
	"math"
	"reflect"
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
	if !reflect.DeepEqual(got, in) {
		t.Fatalf("claims = %+v, want %+v", got, in)
	}
}

func TestVerifyPreservesScopedGrant(t *testing.T) {
	pub, priv := keys(t)
	now := time.Unix(1757000000, 0)
	in := Claims{CID: "1000", Rating: 5, MaxTX: 1, Role: "controller", Callsign: "ZSPD_TWR", TX: []int{118500}, Exp: now.Add(time.Minute).Unix()}
	tok, err := Sign(priv, in)
	if err != nil {
		t.Fatal(err)
	}
	got, err := Verify(pub, tok, now)
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(got, in) {
		t.Fatalf("claims = %+v, want %+v", got, in)
	}
}

func TestVerifyAcceptsAReceiveOnlyListenerGrant(t *testing.T) {
	pub, priv := keys(t)
	now := time.Unix(1757000000, 0)
	in := Claims{CID: "listener-1000", Rating: 1, MaxTX: 0, Role: "listener", Exp: now.Add(time.Minute).Unix()}
	tok, err := Sign(priv, in)
	if err != nil {
		t.Fatal(err)
	}
	got, err := Verify(pub, tok, now)
	if err != nil {
		t.Fatalf("Verify: %v", err)
	}
	if !reflect.DeepEqual(got, in) {
		t.Fatalf("claims = %+v, want %+v", got, in)
	}
}

func TestVerifyRejectsMalformedSignedVoiceScopes(t *testing.T) {
	pub, priv := keys(t)
	now := time.Unix(1757000000, 0)
	for _, tc := range []struct {
		name  string
		claim Claims
	}{
		{"unknown role", Claims{Role: "administrator"}},
		{"controller missing callsign", Claims{Role: "controller", TX: []int{118500}}},
		{"controller with ATIS station", Claims{Role: "controller", Callsign: "ZSPD_TWR", Station: "ZSPD_ATIS", TX: []int{118500}}},
		{"duplicate grant", Claims{Role: "controller", Callsign: "ZSPD_TWR", TX: []int{118500, 118500}}},
		{"out of band grant", Claims{Role: "atis", Station: "ZSPD_ATIS", TX: []int{200000}}},
		{"listener transmit grant", Claims{Role: "listener", MaxTX: 1, TX: []int{118500}}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			c := tc.claim
			c.CID, c.Rating, c.MaxTX, c.Exp = "1000", 5, 2, now.Add(time.Minute).Unix()
			tok, err := Sign(priv, c)
			if err != nil {
				t.Fatal(err)
			}
			if _, err := Verify(pub, tok, now); !errors.Is(err, ErrInvalid) {
				t.Fatalf("Verify error = %v, want ErrInvalid", err)
			}
		})
	}
}

func TestVerifyAcceptsOneSignedPilotFrequency(t *testing.T) {
	pub, priv := keys(t)
	now := time.Unix(1757000000, 0)
	in := Claims{CID: "1000", Rating: 5, MaxTX: 1, Role: "pilot", TX: []int{118500}, Exp: now.Add(time.Minute).Unix()}
	tok, err := Sign(priv, in)
	if err != nil {
		t.Fatal(err)
	}
	got, err := Verify(pub, tok, now)
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(got, in) {
		t.Fatalf("pilot claim = %+v, want %+v", got, in)
	}
}

func TestVerifyRejectsAnExpiredToken(t *testing.T) {
	pub, priv := keys(t)
	now := time.Unix(1757000000, 0)
	// 过期得足够多，落在 clockSkew 容差之外——容差之内的那一段由
	// TestVerifyToleratesASmallClockSkew 专门覆盖。
	tok, _ := Sign(priv, Claims{CID: "1000", Exp: now.Add(-time.Minute).Unix()})
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

// 有效期是右开区间，右端是 exp + clockSkew：到点即过期，没有中间态。
// 这是刻意选择而非疏漏——见 token.go 里 Verify 的注释。
//
// 断言落在**加上容差之后**的那个边界上，而不是 exp 本身：容差存在之后，
// `exp == now` 是合法的（时钟快了 0 秒还是快了 4 秒，我们都认），
// 拿它当"必须被拒"的样例只会把容差自己判红。
func TestVerifyRejectsATokenExpiringExactlyAtTheSkewBoundary(t *testing.T) {
	pub, priv := keys(t)
	now := time.Unix(1757000000, 0)
	tok, _ := Sign(priv, Claims{CID: "1000", Exp: now.Add(-clockSkew).Unix()})
	_, err := Verify(pub, tok, now)
	if err == nil {
		t.Fatal("Verify must reject a token whose exp is exactly clockSkew in the past; the validity window is half-open")
	}
	if !errors.Is(err, ErrExpired) {
		t.Fatalf("err = %v, want errors.Is(err, ErrExpired)", err)
	}
}

// TestVerifyToleratesASmallClockSkew 钉住容差本身。
//
// 它防的是一种全网同时发生的故障：服务端时钟比真实时间快，于是每一张刚签出来的
// token 一到这里就已经"过期"，所有人被以 token_expired 拒掉，而客户端照着这个
// 原因串去 can-api 换新的——换回来的还是过期的。
//
// 输入必须落在 exp 之后、exp+clockSkew 之前：exp 本身或更早的样例分辨不出
// "有容差"和"没容差"（前者两种实现都接受，后者两种实现都拒绝）。
func TestVerifyToleratesASmallClockSkew(t *testing.T) {
	pub, priv := keys(t)
	now := time.Unix(1757000000, 0)
	tok, err := Sign(priv, Claims{CID: "1000", Rating: 1, Exp: now.Add(-clockSkew + time.Second).Unix()})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	if _, err := Verify(pub, tok, now); err != nil {
		t.Fatalf("Verify: %v — a token that expired less than clockSkew ago must still be accepted, or a server clock running a few seconds fast turns away the entire network", err)
	}
}

// TestVerifyRejectsATokenThatLastsTooLong 钉住有效期的上界。
//
// 短有效期是这套设计里**唯一**的吊销机制（本包禁止任何网络调用），所以一张
// exp 写在一千年后的 token 是一张永远吊销不掉的凭据。没有这道闸时
// `exp = now + 1000 年` 和 `exp = 2^62` 都验得过。
//
// 而且它必须是 ErrInvalid 而不是 ErrExpired：两者给客户端的指示相反——
// ErrExpired 的意思是"去换一张新的再来"，而对这种 token 来说换回来的是同样一张。
func TestVerifyRejectsATokenThatLastsTooLong(t *testing.T) {
	pub, priv := keys(t)
	now := time.Unix(1757000000, 0)
	for name, exp := range map[string]int64{
		// 秒数直接算，不走 time.Duration：一千年是 3.15e19 纳秒，超出 int64。
		"a thousand years":    now.Unix() + 1000*365*24*3600,
		"2^62":                1 << 62,
		"the largest int64":   math.MaxInt64,
		"just past the bound": now.Add(maxTokenLifetime + clockSkew + time.Second).Unix(),
	} {
		t.Run(name, func(t *testing.T) {
			tok, err := Sign(priv, Claims{CID: "1000", Rating: 1, Exp: exp})
			if err != nil {
				t.Fatalf("Sign: %v", err)
			}
			_, err = Verify(pub, tok, now)
			if err == nil {
				t.Fatalf("Verify accepted a token valid until %d; a short lifetime is the only revocation this design has", exp)
			}
			if !errors.Is(err, ErrInvalid) {
				t.Fatalf("err = %v, want errors.Is(err, ErrInvalid) — telling the client it merely expired sends it back for a token that will be just as unacceptable", err)
			}
		})
	}
}

// TestVerifyAcceptsATokenAtTheLifetimeBound 是上一条的对照。
//
// 一个把**所有** token 都判成"太长"的变异体，靠上面那张全是"应该失败"的表
// 是抓不住的——它们全部还是会通过。
func TestVerifyAcceptsATokenAtTheLifetimeBound(t *testing.T) {
	pub, priv := keys(t)
	now := time.Unix(1757000000, 0)
	tok, err := Sign(priv, Claims{CID: "1000", Rating: 1, Exp: now.Add(maxTokenLifetime).Unix()})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	if _, err := Verify(pub, tok, now); err != nil {
		t.Fatalf("Verify: %v — a token exactly at the permitted lifetime must be accepted", err)
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

// TestAnUnverifiedPayloadIsNeverParsed 钉住 Verify 里那个顺序：**先验签名，
// 再解析载荷**。
//
// 这条性质是本包最被称道的那个，而在此之前没有任何东西守着它：把
// json.Unmarshal 那三行提到 ed25519.Verify 前面，整套测试照绿——别的测试要么
// 用签名合法的 token（两种顺序结果相同），要么用畸形到根本走不到那一步的输入。
//
// 探针必须**同时**满足两件事，一件都不能少：签名验不过，而载荷是解析器会拒绝
// 的东西。于是两种顺序给出不同的答案，而答案是唯一看得见的区别——
//
//	正确顺序：token signature does not verify   （压根没碰过那些字节）
//	颠倒顺序：token payload is not valid JSON   （先把攻击者可控的字节喂给了解析器）
//
// 为什么值得钉：验签之前 token 里的每个字节都是攻击者随手可写的输入，而
// encoding/json 是一大片解析代码。今天它很稳，但"在验证之前不要解析不可信
// 输入"是一条一旦丢掉就再也没人会注意到的纪律——因为两种写法在所有正常输入上
// 的行为完全一致。
func TestAnUnverifiedPayloadIsNeverParsed(t *testing.T) {
	pub, _ := keys(t)

	// 载荷是合法 base64url，但解出来不是 JSON；签名长度合法，但是一串零，
	// 对任何公钥都验不过。
	body := enc.EncodeToString([]byte("this is not JSON at all"))
	tok := body + "." + enc.EncodeToString(make([]byte, ed25519.SignatureSize))

	_, err := Verify(pub, tok, time.Unix(1757000000, 0))
	if err == nil {
		t.Fatal("Verify accepted a token with an all-zero signature")
	}
	if !errors.Is(err, ErrInvalid) {
		t.Fatalf("Verify returned %v, want it wrapped in ErrInvalid", err)
	}
	if !strings.Contains(err.Error(), "signature does not verify") {
		t.Fatalf("Verify failed with %q, want the signature check to be what rejected it — this token's payload was fed to encoding/json before anything proved the bytes came from can-api", err)
	}
}

// TestVerifyRejectsATokenWithNoCid 钉住空 cid 那道闸。
//
// 它此前**零覆盖**：整段 `if c.CID == "" { … }` 删掉，全套测试照绿。而
// router.Add 明确依赖它——那里有一段
// `if o.CID != "" { … 顶号 … }`，注释写着"空 CID 不参与顶号。它不该出现
// （鉴权拒绝空 CID）"。也就是说这道闸一旦没了，router 不会报错，它会**按设计
// 放行**：每一条空 cid 的会话都登记成功、互不顶替，而顶号正是这套设计里
// 防"一个人两条会话听见自己回声"的那一条。一张 `{"cid":""}` 的 token
// （签发方少填一个字段就是）于是能开任意多条并存的会话。
//
// 落在 ErrInvalid 而不是 ErrExpired：换一张新票不会让它长出 cid 来。
func TestVerifyRejectsATokenWithNoCid(t *testing.T) {
	pub, priv := keys(t)
	now := time.Unix(1757000000, 0)
	tok, err := Sign(priv, Claims{Rating: 5, MaxTX: 8, Exp: now.Add(60 * time.Second).Unix()})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	// 前提：除了 cid 以外这张票完全合格——所以红的时候只可能是这道闸没了。
	if _, err := Verify(pub, mustSign(t, priv, Claims{CID: "1000", Rating: 5, MaxTX: 8, Exp: now.Add(60 * time.Second).Unix()}), now); err != nil {
		t.Fatalf("premise: the same token with a cid does not verify either: %v", err)
	}
	_, err = Verify(pub, tok, now)
	if !errors.Is(err, ErrInvalid) {
		t.Fatalf("Verify with an empty cid = %v, want ErrInvalid — router.Add deliberately skips eviction for an empty cid (\"it must not happen, auth refuses it\"), so without this gate every such session is admitted and none can evict another", err)
	}
}

func mustSign(t *testing.T, priv ed25519.PrivateKey, c Claims) string {
	t.Helper()
	tok, err := Sign(priv, c)
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	return tok
}
