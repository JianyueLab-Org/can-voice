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

// maxTokenLifetime 是 exp 允许比现在远出多少。超过就按 ErrInvalid 拒。
//
// 为什么必须有这道闸：**短有效期就是这套设计里唯一的吊销机制**。本包的包注释
// 明令禁止任何网络调用（不查撤销列表、不拉公钥、不做 introspection），所以一张
// 已经签出去的 token 在它过期之前没有任何办法作废。没有上界的话，`exp` 写成
// `now + 1000 年` 或者 `2^62` 都验得过——一次签发方的失误（一个写错的
// `expires_in`、一张为了调试手签的长 token 漏进生产）就是一张**永久**有效、
// 永远吊销不掉的凭据，而且这边完全看不出来。
//
// 取值的来历：设计文档 §6 写的是 can-api 签 60 秒的 token（`expires_in: 60`）。
// 这里取它的十倍，而不是恰好 60 秒——签发方将来可能为"掉线重连不必重新取票"
// 之类的理由把有效期放宽一点，那不该要求服务端跟着改一次代码；而十分钟已经
// 足够短：一张误签的 token 最坏也只是十分钟之后自己消失，不是永远。
const maxTokenLifetime = 10 * time.Minute

// clockSkew 是验期时容忍的两端时钟偏差。
//
// 它防的是一种**全网同时发生**的故障：服务端的时钟比真实时间快，于是每一张
// 刚签出来的 token 一到这里就已经"过期"，所有人被以 token_expired 拒掉，而
// 客户端照着这个原因串去换新 token——换回来的还是过期的。整个网络下线，症状
// 却指向 can-api。
//
// 取 5 秒，刻意远小于 60 秒的有效期：容差是在有效期尾巴上多接受一小段，所以它
// 直接削弱吊销窗口，给大了等于偷偷把 token 寿命拉长。同步过 NTP 的主机偏差在
// 毫秒级，5 秒覆盖的是"这台机器根本没在同步"那类事故的起头一段。它不是万能的
// ——偏差超过一整个有效期时仍然会全网拒绝，那种情况要靠日志和 README 的排障
// 条目认出来，而不是靠把容差调大。
const clockSkew = 5 * time.Second

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

	// 有效期上界先判，顺序有意义：一张 exp 写成一千年后的 token 既"没过期"又
	// 不可接受，而这两条给客户端的指示相反——ErrExpired 的意思是"去换一张新的
	// 再来"，对这种 token 来说换回来的会是同样一张。所以它必须落在 ErrInvalid。
	//
	// 比的是整数秒而不是 time.Time：`exp` 是对端给的任意 int64，
	// time.Unix(math.MaxInt64, 0) 内部会溢出，比较结果不再有意义。
	if c.Exp > now.Add(maxTokenLifetime+clockSkew).Unix() {
		return Claims{}, fmt.Errorf("%w: token is valid until %d, which is more than %s from now (%d)",
			ErrInvalid, c.Exp, maxTokenLifetime, now.Unix())
	}

	// exp 是右开区间的上界：到点即过期。这样"有效期 60 秒"的 token 不会在恰好
	// 第 60 秒这个瞬间产生"到底算不算过期"的歧义——一个 token 要么在窗口内，
	// 要么不在，没有中间态。区间的右端现在是 exp + clockSkew（见 clockSkew），
	// 等价地：把 now 往回拨 clockSkew 再比。
	if c.Exp <= now.Add(-clockSkew).Unix() {
		return Claims{}, fmt.Errorf("%w: token expired at %d, now is %d", ErrExpired, c.Exp, now.Unix())
	}
	if c.CID == "" {
		return Claims{}, fmt.Errorf("%w: token carries no cid", ErrInvalid)
	}
	return c, nil
}
