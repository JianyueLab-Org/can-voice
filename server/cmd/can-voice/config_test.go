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
		_, err := LoadConfig(env(full))
		if err == nil {
			t.Fatalf("LoadConfig must fail when %s is missing", missing)
		}
		// 断言必须是 "<变量> is required" 这个精确短语，而不是只看错误信息里
		// 有没有出现变量名。CAN_VOICE_API_PUBKEY 有自己专门的判空检查，但空串
		// 一样能被 base64 解码成 0 字节，随后下游的长度检查也会报错，而那条
		// 错误信息里同样包含 "CAN_VOICE_API_PUBKEY"（"...decodes to 0 bytes..."）。
		// 只查子串的话，删掉专门的判空检查、只留长度检查，测试照样绿——
		// 这正是本仓库"一个断言只有在能分辨两种实现时才算数"那条规则要求排除的
		// 假阳性。四个变量共用同一个 "%s is required" 格式串，所以统一按这个
		// 精确短语断言，比再给 PUBKEY 开一个专门的测试函数更省，也一样能把
		// 四个判空检查分别钉住。
		want := missing + " is required"
		if !strings.Contains(err.Error(), want) {
			t.Fatalf("error must say %q, got: %v", want, err)
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

func TestLoadConfigRejectsInvalidMaxRX(t *testing.T) {
	// n <= 0 静默地把 RX 上限砍成 0 或负数——不是配置错误弹出来，
	// 而是启动干净地成功、日志毫无异常，然后没有人能订阅到任何频率。
	// 这正是"宁可起不来，也不要带着半份配置跑"这套设计想防的那类失败，
	// 所以非法输入必须让 LoadConfig 失败，而不是被 strconv.Atoi 悄悄吞掉。
	for _, v := range []string{"abc", "0", "-5"} {
		t.Run(v, func(t *testing.T) {
			_, err := LoadConfig(env(map[string]string{
				"CAN_VOICE_ADDR":       ":64738",
				"CAN_VOICE_TLS_CERT":   "/tmp/c.pem",
				"CAN_VOICE_TLS_KEY":    "/tmp/k.pem",
				"CAN_VOICE_API_PUBKEY": "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=",
				"CAN_VOICE_MAX_RX":     v,
			}))
			if err == nil {
				t.Fatalf("LoadConfig must reject CAN_VOICE_MAX_RX=%q", v)
			}
			if !strings.Contains(err.Error(), "CAN_VOICE_MAX_RX") {
				t.Fatalf("error must name CAN_VOICE_MAX_RX, got: %v", err)
			}
		})
	}
}

func TestLoadConfigHonoursAMaxRXOverride(t *testing.T) {
	// 正面用例不能省：一个把 CAN_VOICE_MAX_RX 不管什么值都拒绝的变异体，
	// 光靠上面那几个"应该失败"的用例是测不出来的——它们全部还是会通过。
	cfg, err := LoadConfig(env(map[string]string{
		"CAN_VOICE_ADDR":       ":64738",
		"CAN_VOICE_TLS_CERT":   "/tmp/c.pem",
		"CAN_VOICE_TLS_KEY":    "/tmp/k.pem",
		"CAN_VOICE_API_PUBKEY": "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=",
		"CAN_VOICE_MAX_RX":     "64",
	}))
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}
	if cfg.MaxRX != 64 {
		t.Fatalf("MaxRX = %d, want the 64 override", cfg.MaxRX)
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

// TestLoadConfigRejectsAnOutOfRangeMaxRX 钉住 MaxRX 的上界。
//
// router 里 maxRejected 那段算术明说它依赖这个值。配到几千之后 SUBACK 会超过
// 64 KiB 的帧上限而根本发不出去——一条**已经生效**的 SUB 得不到任何回应，
// 客户端只能一遍遍重发同一份声明，两端都没有一行日志说出这件事。
func TestLoadConfigRejectsAnOutOfRangeMaxRX(t *testing.T) {
	for _, v := range []string{"1025", "100000"} {
		t.Run(v, func(t *testing.T) {
			_, err := LoadConfig(env(map[string]string{
				"CAN_VOICE_ADDR":       ":64738",
				"CAN_VOICE_TLS_CERT":   "/tmp/c.pem",
				"CAN_VOICE_TLS_KEY":    "/tmp/k.pem",
				"CAN_VOICE_API_PUBKEY": "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=",
				"CAN_VOICE_MAX_RX":     v,
			}))
			if err == nil {
				t.Fatalf("LoadConfig must reject CAN_VOICE_MAX_RX=%q; the SUBACK for that many frequencies does not fit a control frame", v)
			}
			if !strings.Contains(err.Error(), "CAN_VOICE_MAX_RX") {
				t.Fatalf("error must name CAN_VOICE_MAX_RX, got: %v", err)
			}
		})
	}

	// 对照：恰好在上界上的值必须**通过**。没有这一条的话，一个把上界写成 1 的
	// 实现照样让上面两个子用例绿。
	cfg, err := LoadConfig(env(map[string]string{
		"CAN_VOICE_ADDR":       ":64738",
		"CAN_VOICE_TLS_CERT":   "/tmp/c.pem",
		"CAN_VOICE_TLS_KEY":    "/tmp/k.pem",
		"CAN_VOICE_API_PUBKEY": "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=",
		"CAN_VOICE_MAX_RX":     "1024",
	}))
	if err != nil {
		t.Fatalf("LoadConfig rejected CAN_VOICE_MAX_RX at exactly the limit: %v", err)
	}
	if cfg.MaxRX != maxMaxRX {
		t.Fatalf("MaxRX = %d, want %d", cfg.MaxRX, maxMaxRX)
	}
}

// 兜底半径表可以配，而且配错了**起不来**。
//
// 悄悄回退到内置那张估出来的表的话，一个打错了一个字符的运维以为自己校准过了
// ——而没有任何地方会告诉他服务端跑的还是旧数值。
func TestLoadConfigHonoursTheSuffixRangeOverride(t *testing.T) {
	base := map[string]string{
		"CAN_VOICE_ADDR":       ":64738",
		"CAN_VOICE_TLS_CERT":   "/tmp/c.pem",
		"CAN_VOICE_TLS_KEY":    "/tmp/k.pem",
		"CAN_VOICE_API_PUBKEY": "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=",
	}
	with := func(spec string) map[string]string {
		m := make(map[string]string, len(base)+1)
		for k, v := range base {
			m[k] = v
		}
		m["CAN_VOICE_SUFFIX_RANGES"] = spec
		return m
	}

	cfg, err := LoadConfig(env(with("CTR=300")))
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}
	if got := cfg.Ranges.RangeNM("ZBPE_CTR"); got != 300 {
		t.Errorf("CTR = %v, want the 300 override", got)
	}
	if got := cfg.Ranges.RangeNM("ZSPD_TWR"); got != 30 {
		t.Errorf("TWR = %v, want the built-in 30", got)
	}

	if _, err := LoadConfig(env(with("CTR=nope"))); err == nil {
		t.Error("LoadConfig accepted an unparsable suffix range table")
	}

	// 没配过的服务端照样起得来，用的是内置那一份。
	cfg, err = LoadConfig(env(base))
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}
	if got := cfg.Ranges.RangeNM("ZSPD_TWR"); got != 30 {
		t.Errorf("TWR = %v, want the built-in 30", got)
	}
}
