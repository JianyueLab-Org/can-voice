package main

import (
	"crypto/ed25519"
	"encoding/base64"
	"fmt"
	"strconv"

	"github.com/JianyueLab-Org/can-voice/server/internal/geo"
)

// Config 是进程的全部配置。
//
// 全部来自环境变量，没有配置文件，缺任何必需项就启动失败——
// 与 can-api 同样的做法。一个语音服务端带着半份配置跑起来，
// 比根本起不来危险得多。
type Config struct {
	Addr    string
	Cert    string
	Key     string
	PubKey  ed25519.PublicKey
	FeedURL string
	MaxRX   int
	// Ranges 是席位后缀的兜底半径表。只在 datafeed 的 visual_range 为 0 时用得上。
	Ranges *geo.Table
}

// defaultFeedURL 是 can-fsd 的 SSE 流。射程过滤的输入就来自这里；
// 连不上时服务端降级为不做射程过滤，而不是拒绝服务（spec 7.3）。
const defaultFeedURL = "https://data.ceruleanavi.net/v1/events"

// LoadConfig 从一个取环境变量的函数里读配置。
// 传函数而不是直接读 os.Getenv，是为了让它可以被测试。
func LoadConfig(get func(string) string) (Config, error) {
	var cfg Config
	for _, req := range []struct {
		name string
		dst  *string
	}{
		{"CAN_VOICE_ADDR", &cfg.Addr},
		{"CAN_VOICE_TLS_CERT", &cfg.Cert},
		{"CAN_VOICE_TLS_KEY", &cfg.Key},
	} {
		v := get(req.name)
		if v == "" {
			return Config{}, fmt.Errorf("%s is required", req.name)
		}
		*req.dst = v
	}

	raw := get("CAN_VOICE_API_PUBKEY")
	if raw == "" {
		return Config{}, fmt.Errorf("CAN_VOICE_API_PUBKEY is required")
	}
	key, err := base64.StdEncoding.DecodeString(raw)
	if err != nil {
		return Config{}, fmt.Errorf("CAN_VOICE_API_PUBKEY is not base64: %w", err)
	}
	// Ed25519 公钥恰好 32 字节。长度不对却能解码的话每次验签都会失败，
	// 而错误信息会指向 token 而不是这份配置。
	if len(key) != ed25519.PublicKeySize {
		return Config{}, fmt.Errorf(
			"CAN_VOICE_API_PUBKEY decodes to %d bytes, an Ed25519 public key is %d",
			len(key), ed25519.PublicKeySize)
	}
	cfg.PubKey = ed25519.PublicKey(key)

	cfg.FeedURL = get("CAN_VOICE_FSD_FEED")
	if cfg.FeedURL == "" {
		cfg.FeedURL = defaultFeedURL
	}
	cfg.MaxRX = 32
	if v := get("CAN_VOICE_MAX_RX"); v != "" {
		n, err := strconv.Atoi(v)
		if err != nil || n <= 0 {
			return Config{}, fmt.Errorf("CAN_VOICE_MAX_RX must be a positive integer, got %q", v)
		}
		if n > maxMaxRX {
			return Config{}, fmt.Errorf("CAN_VOICE_MAX_RX is %d, over the %d limit", n, maxMaxRX)
		}
		cfg.MaxRX = n
	}

	// 兜底半径表。逐条覆盖内置那份估出来的数值，形如 `CTR=300,FSS=700,*=120`。
	// **写坏了起不来**：悄悄回退的话，一个打错了一个字符的运维以为自己校准过了。
	ranges, err := geo.ParseTable(get("CAN_VOICE_SUFFIX_RANGES"))
	if err != nil {
		return Config{}, fmt.Errorf("CAN_VOICE_SUFFIX_RANGES: %w", err)
	}
	cfg.Ranges = ranges

	return cfg, nil
}

// maxMaxRX 是 CAN_VOICE_MAX_RX 的上界。
//
// 有上界，因为 router 里 maxRejected 那段算术**明说**它依赖这个值："RX/TX 的
// 长度由 MaxRX/MaxTX 决定……真要把 MaxRX 配到几千，这个上界要重算。" 一份
// SUBACK 要塞进 64 KiB 的控制帧：固定部分加 RejectedXC 约 5.8 KB，剩下约 59.7 KB
// 给 ack.RX 加 ack.TX，按每个频率 11 字节算是约 5400 个。传输层已经把 MaxTX 夹到
// MaxRX 以内（grantedMaxTX），所以 RX+TX 最多 2×MaxRX，于是安全线在 2700 上下。
//
// 取 1024，离那条线还有两倍半的余量，而且远远超出任何真实用法——一个管制员
// 同时监听的频率是几十个量级。配大了的后果不是"慢一点"：SUBACK 一旦超过帧上限
// 就根本发不出去，一条**已经生效**的 SUB 得不到任何回应，而客户端只能一遍遍
// 重发同一份声明。
//
// 与其让那条路在生产上被一个手滑的环境变量踩出来，不如启动就失败——这个进程
// 的全部配置哲学就是"宁可起不来，也不要带着半份配置跑"。
const maxMaxRX = 1024
