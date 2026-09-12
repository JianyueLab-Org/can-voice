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
// 方括号那一支不再只认十六进制数字+冒号：修复轮 1 用 [0-9a-fA-F:]+ 时，
// 带 zone id 的链路本地地址（如 "[fe80::1%en0]"）里的 "%en0" 是网卡名，
// 不在那个字符集里，导致整个方括号原样漏网——而网卡名恰恰是这次要堵的
// 那一类信息。zone id 的合法字符集本身没有列全的办法（Windows 上是数字
// 索引，*nix 上是任意网卡名），所以不再枚举，改成方括号里只要不是空白、
// 不是点号、不是右方括号本身，就当地址内容处理：Go 只会给地址加方括号，
// 方括号里出现的字母数字和冒号原样清洗是安全的。
// 唯独保留排除点号（.）：这不是疏漏，而是为了不吞掉像
// "[::ffff:192.168.1.23]" 这种内嵌了点分十进制 IPv4 的写法——那种情况下
// 方括号本身应该原样保留，只让下面的 IPv4 分支单独抹掉里面的点分数字，
// 断了这条路会让整个方括号（连同后面的端口）被吞成一个 <addr>，丢失
// "还留着方括号结构" 这种诊断价值；`TestScrubAddrs` 里的
// "IPv4 映射地址" 用例把这一点钉住。
// IPv6 要求带方括号（Go 的 *net.OpError.Error() 就是这么拼的）。
var addrPattern = regexp.MustCompile(`\[[0-9a-zA-Z:%]+\](:\d+)?|\b(?:\d{1,3}\.){3}\d{1,3}\b(:\d+)?`)

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
