package main

import (
	"context"
	"crypto/tls"
	"regexp"
	"time"

	"github.com/quic-go/quic-go"
)

const ALPN = "can-voice-probe/1"

// addrPattern 匹配错误文本里可能出现的 IP 地址，带端口的和不带端口的都算。
//
// 方括号那一支不枚举内容字符：只要求"不是空白、不是右方括号本身"。
// Go 只会给地址加方括号（*net.OpError.Error() 就是这么拼的），所以方括号
// 里的东西整体当地址处理是安全的——不管里面是纯 IPv6、带 zone id 的链路
// 本地地址（"[fe80::1%en0]"，网卡名），还是 IPv4 映射写法
// （"[::ffff:192.168.1.23]"）。
//
// 这里特意不去挑一个"允许的字符集合"（哪怕是十六进制+字母+% 这种更宽的
// 集合）：zone id 本身就没有一个能列全的合法字符集——*nix 上是任意网卡名，
// 可以带点号（VLAN 子接口名 "eth0.100"），Windows 上是数字索引——任何一版
// 枚举都会在某个真实系统上漏一次。方括号本身已经是 Go 给的边界，没有必要
// 在它内部再挑字符：只要不是空白、不是收尾的 ]，就整体替换。
// 过度清洗是安全的失败方向，清洗不足才是泄露——所以宁可让
// "[::ffff:192.168.1.23]:54321" 整段变成一个 <addr>（丢了"这是方括号
// 结构"这条诊断细节），也不要在字符集里再挑一次可能漏的集合。
var addrPattern = regexp.MustCompile(`\[[^\]\s]+\](:\d+)?|\b(?:\d{1,3}\.){3}\d{1,3}\b(:\d+)?`)

// scrubAddrs 抹掉错误文本里的 IP 地址。
// 握手失败时，socket 级错误（no route to host / network is unreachable 等）在 Go 里是
// *net.OpError，它的 Error() 会把本机和对端的 "IP:端口" 都拼进文本，例如：
// "dial udp 192.168.1.23:54321->203.0.113.7:4433: connect: no route to host"
// 其中 192.168.1.23 是测试用户的内网地址。这段文本会原样进入 HandshakeResult.Error，
// 最终写进 Task 7 生成、由测试用户手动回传的报告文件，而 Task 8 的中文 README
// 明确承诺"不收集任何个人信息"，所以必须在产生处就清洗掉，不能指望下游过滤。
// 过度清洗是安全的失败方向（"context deadline exceeded"、DNS 查找失败这类不含
// IP 的诊断信息不会被误伤），清洗不足才是泄露，所以这里不区分本机和对端，
// 一律替换成字面量 "<addr>"。
func scrubAddrs(s string) string {
	return addrPattern.ReplaceAllString(s, "<addr>")
}

// HandshakeResult 是探针的第一个测量项：QUIC 握手能不能做完。
// 握手失败是最可能的失败形态（UDP 被整体阻断），所以它的错误文本要原样留下。
type HandshakeResult struct {
	OK     bool   `json:"ok"`
	Millis int64  `json:"millis"`
	Error  string `json:"error,omitempty"`
}

// Dial 建立探针连接。无论成败都返回一个填好的 HandshakeResult。
func Dial(ctx context.Context, addr string, insecure bool) (quic.Connection, HandshakeResult, error) {
	start := time.Now()
	conn, err := quic.DialAddr(ctx, addr, &tls.Config{
		InsecureSkipVerify: insecure,
		NextProtos:         []string{ALPN},
	}, &quic.Config{
		EnableDatagrams: true,
		MaxIdleTimeout:  90 * time.Second,
		KeepAlivePeriod: 15 * time.Second,
	})
	res := HandshakeResult{Millis: time.Since(start).Milliseconds()}
	if res.Millis == 0 {
		res.Millis = 1 // 保证"尝试过"和"没尝试"可区分
	}
	if err != nil {
		res.Error = scrubAddrs(err.Error())
		return nil, res, err
	}
	res.OK = true
	return conn, res, nil
}
