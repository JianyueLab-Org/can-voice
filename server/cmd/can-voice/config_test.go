package main

import (
	"strings"
	"testing"
)

func env(m map[string]string) func(string) string {
	return func(k string) string { return m[k] }
}

func TestLoadConfigRequiresEveryEssentialValue(t *testing.T) {
	for _, missing := range []string{"CAN_VOICE_ADDR", "CAN_VOICE_TLS_CERT", "CAN_VOICE_TLS_KEY", "CAN_VOICE_API_PUBKEY"} {
		full := map[string]string{
			"CAN_VOICE_ADDR":       ":64738",
			"CAN_VOICE_TLS_CERT":   "/tmp/c.pem",
			"CAN_VOICE_TLS_KEY":    "/tmp/k.pem",
			"CAN_VOICE_API_PUBKEY": "3q2+7w==",
		}
		delete(full, missing)
		if _, err := LoadConfig(env(full)); err == nil {
			t.Fatalf("LoadConfig must fail when %s is missing", missing)
		} else if !strings.Contains(err.Error(), missing) {
			t.Fatalf("error must name the missing variable %s, got: %v", missing, err)
		}
	}
}

func TestLoadConfigRejectsAnUnparsablePublicKey(t *testing.T) {
	// 这个值是精心挑的，不是随手写的一串垃圾：前 44 个字符是合法 base64
	// （解出 32 字节），末尾那个 '!' 让 DecodeString 在第 44 字节报错——
	// 所以它**同时**返回 32 字节和一个错误。于是去掉 err 检查时，长度检查会放行
	// 一个损坏的公钥，测试才会红。换成 "!!! not base64 !!!" 那种输入就没有牙齿了：
	// 它解出 0 字节，长度检查本来就会拒，两种实现结果一样。
	t.Run("decodes to 32 bytes but still errors", func(t *testing.T) {
		_, err := LoadConfig(env(map[string]string{
			"CAN_VOICE_ADDR":       ":64738",
			"CAN_VOICE_TLS_CERT":   "/tmp/c.pem",
			"CAN_VOICE_TLS_KEY":    "/tmp/k.pem",
			"CAN_VOICE_API_PUBKEY": "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=!",
		}))
		if err == nil {
			t.Fatal("LoadConfig must reject a public key it cannot decode, even when the decoded prefix is exactly 32 bytes")
		}
	})

	// 这一条走的是友好路径：解不出任何字节，长度检查本来就会拒绝它，
	// 所以它测不出 err 检查有没有被删掉。留着只是记录"明显是垃圾"的输入
	// 也确实被拒；真正的钉子是上面那个 subtest。
	t.Run("obviously not base64", func(t *testing.T) {
		_, err := LoadConfig(env(map[string]string{
			"CAN_VOICE_ADDR":       ":64738",
			"CAN_VOICE_TLS_CERT":   "/tmp/c.pem",
			"CAN_VOICE_TLS_KEY":    "/tmp/k.pem",
			"CAN_VOICE_API_PUBKEY": "!!! not base64 !!!",
		}))
		if err == nil {
			t.Fatal("LoadConfig must reject a public key it cannot decode")
		}
	})
}

func TestLoadConfigRejectsAPublicKeyOfTheWrongLength(t *testing.T) {
	// Ed25519 公钥恰好 32 字节。长度不对却能解码的话，
	// 每一次验签都会失败，而错误信息会指向 token 而不是配置。
	_, err := LoadConfig(env(map[string]string{
		"CAN_VOICE_ADDR":       ":64738",
		"CAN_VOICE_TLS_CERT":   "/tmp/c.pem",
		"CAN_VOICE_TLS_KEY":    "/tmp/k.pem",
		"CAN_VOICE_API_PUBKEY": "AAAA",
	}))
	if err == nil {
		t.Fatal("LoadConfig must reject an Ed25519 public key that is not 32 bytes")
	}
}

func TestLoadConfigDefaultsTheOptionalValues(t *testing.T) {
	cfg, err := LoadConfig(env(map[string]string{
		"CAN_VOICE_ADDR":       ":64738",
		"CAN_VOICE_TLS_CERT":   "/tmp/c.pem",
		"CAN_VOICE_TLS_KEY":    "/tmp/k.pem",
		"CAN_VOICE_API_PUBKEY": "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=",
	}))
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}
	if cfg.MaxRX != 32 {
		t.Fatalf("MaxRX = %d, want the 32 default", cfg.MaxRX)
	}
	if cfg.FeedURL == "" {
		t.Fatal("FeedURL must have a default; it is how range filtering gets its input")
	}
}
