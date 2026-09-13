// Package auth 校验 can-api 签发的短期语音 token。
//
// can-voice 持公钥本地验签，不回调 can-api——所以 can-api 挂掉不影响已连接的用户，
// 也不影响持有未过期 token 的重连。这与旧的 Ice 认证器架构相反：那里认证器一挂，
// 每次登录都被拒，而客户端显示的是"密码错误"。token 短寿命就是撤销机制本身——
// 本包永远不应该增加任何网络调用（不查撤销列表、不拉公钥、不做 introspection）。
//
// 格式是 base64url(claims_json) + "." + base64url(signature)。
// 不用 JWT 库：只有一种算法、一种用途，几十行的实现比一个可配置算法的库更难出错——
// JWT 那类 "alg: none" 的洞在这里根本不存在。
package auth

import (
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"time"
)

var (
	// ErrExpired 表示 token 本身没问题，只是过期了。
	// 客户端应当去换一张新的再试；这是唯一值得重试的失败。
	ErrExpired = errors.New("token expired")
	// ErrInvalid 表示 token 形状、签名或内容不对。
	//
	// 对外只区分到这个粒度：告诉一个**尚未鉴权**的对端"是签名长度不对
	// 还是 payload 不是合法 JSON"，只会帮他调试伪造，对正当客户端毫无用处。
	// 详细原因仍然留在错误链里，进服务端日志。
	ErrInvalid = errors.New("token invalid")
)

// Claims 是 token 的载荷。MaxTX 是这个会话允许同时发送的频率数上限，
// 权威值就是这里——控制面 READY 里的同名字段只是回显。
type Claims struct {
	CID    string `json:"cid"`
	Rating int    `json:"rating"`
	MaxTX  int    `json:"max_tx"`
	Exp    int64  `json:"exp"`
}

var enc = base64.RawURLEncoding

// Sign 签发一个 token。生产环境里签发由 can-api 完成（持有私钥的一方）；
// 这个函数只供测试使用，同时作为 can-api 那边参考实现的样板——can-voice
// 服务器本身永远只验签，从不签发。
func Sign(priv ed25519.PrivateKey, c Claims) (string, error) {
	payload, err := json.Marshal(c)
	if err != nil {
		return "", err
	}
	body := enc.EncodeToString(payload)
	sig := ed25519.Sign(priv, []byte(body))
	return body + "." + enc.EncodeToString(sig), nil
}

// Verify 校验签名与有效期，返回载荷。now 由调用方传入而不是内部调用
// time.Now()，这样过期逻辑才可测试。
//
// 顺序是刻意的：先验证签名，再解析 JSON，最后才看 exp——在验签之前，
// token 里的每个字节都是攻击者可控的输入，把它们喂给 JSON 解析器或拿去
// 判断是否过期，都是在信任未经验证的数据。
func Verify(pub ed25519.PublicKey, token string, now time.Time) (Claims, error) {
	if len(pub) != ed25519.PublicKeySize {
		// 刻意不包 ErrInvalid：这一条说的是**服务端自己配错了公钥**，
		// 不是对端的 token 有问题。包上去的话，一个配置事故会对每一个
		// 正当客户端回报 "token_invalid"，把所有人送去重新取 token。
		return Claims{}, fmt.Errorf("public key has wrong length: %d", len(pub))
	}

	body, sigPart, ok := strings.Cut(token, ".")
	if !ok || body == "" || sigPart == "" {
		return Claims{}, fmt.Errorf("%w: token is not in the <payload>.<signature> form", ErrInvalid)
	}
	if strings.Contains(sigPart, ".") {
		return Claims{}, fmt.Errorf("%w: token has more than two parts", ErrInvalid)
	}

	sig, err := enc.DecodeString(sigPart)
	if err != nil {
		return Claims{}, fmt.Errorf("%w: token signature is not base64url: %w", ErrInvalid, err)
	}
	// ed25519.Verify 要求签名恰好 64 字节，否则 panic；这里提前挡掉任何
	// 长度不对的输入，让畸形 token 变成一个普通的错误返回而不是崩溃。
	if len(sig) != ed25519.SignatureSize {
		return Claims{}, fmt.Errorf("%w: token signature has wrong length: %d", ErrInvalid, len(sig))
	}

	// 先验签，再信任 body 里的任何字节。
	if !ed25519.Verify(pub, []byte(body), sig) {
		return Claims{}, fmt.Errorf("%w: token signature does not verify", ErrInvalid)
	}

	payload, err := enc.DecodeString(body)
	if err != nil {
		// 签名验过了但 payload 解不出来——不应该发生（body 参与了签名),
		// 但仍然按错误处理而不是 panic。
		return Claims{}, fmt.Errorf("%w: token payload is not base64url: %w", ErrInvalid, err)
	}
	var c Claims
	if err := json.Unmarshal(payload, &c); err != nil {
		return Claims{}, fmt.Errorf("%w: token payload is not valid JSON: %w", ErrInvalid, err)
	}

	// exp 是右开区间的上界：now == exp 视为已过期。这样"有效期 60 秒"的
	// token 不会在恰好第 60 秒这个瞬间产生"到底算不算过期"的歧义——
	// 一个 token 要么在窗口内（now < exp），要么不在，没有中间态。
	if !now.Before(time.Unix(c.Exp, 0)) {
		return Claims{}, fmt.Errorf("%w: token expired at %d, now is %d", ErrExpired, c.Exp, now.Unix())
	}
	if c.CID == "" {
		return Claims{}, fmt.Errorf("%w: token carries no cid", ErrInvalid)
	}
	return c, nil
}
