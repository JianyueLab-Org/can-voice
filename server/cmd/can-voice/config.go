package main

import (
	"crypto/ed25519"
	"encoding/base64"
	"fmt"
	"strconv"
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
		cfg.MaxRX = n
	}
	return cfg, nil
}
