package main

import (
	"context"
	"strings"
	"testing"
	"time"
)

func TestDialReportsFailureWithoutReturningAnError(t *testing.T) {
	// 127.0.0.1:1 上没有任何东西在听，握手必然失败。
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()

	_, res, err := Dial(ctx, "127.0.0.1:1", true)
	if err == nil {
		t.Fatal("Dial to a dead address must return an error")
	}
	if res.OK {
		t.Fatal("HandshakeResult.OK must be false when the handshake failed")
	}
	if res.Error == "" {
		t.Fatal("HandshakeResult.Error must carry the reason; it is the whole point of the probe")
	}
	if res.Millis <= 0 {
		t.Fatal("HandshakeResult.Millis must record how long the attempt took")
	}
}

func TestScrubAddrs(t *testing.T) {
	cases := []struct {
		name string
		in   string
		want string
	}{
		{
			name: "IPv4 加端口的真实 OpError 文本（本机+对端各一个）",
			in:   "dial udp 192.168.1.23:54321->203.0.113.7:4433: connect: no route to host",
			want: "dial udp <addr>-><addr>: connect: no route to host",
		},
		{
			name: "裸 IPv4，不带端口",
			in:   "no route to host: 192.168.1.5",
			want: "no route to host: <addr>",
		},
		{
			name: "带方括号的 IPv6 加端口",
			in:   "dial udp [fe80::1]:54321: connect: network is unreachable",
			want: "dial udp <addr>: connect: network is unreachable",
		},
		{
			// 修复轮 2：round 1 的字符集 [0-9a-fA-F:] 不含 "%"，
			// 链路本地地址的 zone id（网卡名 en0）原样漏网。
			name: "带 zone id 的链路本地 IPv6，两边地址都要抹",
			in:   "dial udp [fe80::1%en0]:54321->[2001:db8::1]:4433: connect: network is unreachable",
			want: "dial udp <addr>-><addr>: connect: network is unreachable",
		},
		{
			name: "IPv4 加 :0 端口的监听错误",
			in:   "listen udp 0.0.0.0:0: socket: too many open files",
			want: "listen udp <addr>: socket: too many open files",
		},
		{
			// 修复轮 3：round 2 曾经把这条钉成"方括号结构保留，只抹内嵌数字"
			// （[::ffff:<addr>]:54321），但那是协调者把 round 1 的观察误写成
			// 了"必须保持"，不是真实要求。方括号那一支不再区分内容，整段
			// （连同端口）替换成一个 <addr> 才是过度清洗、更干净的结果。
			name: "方括号里内嵌点分十进制的 IPv4 映射地址，整段抹掉",
			in:   "[::ffff:192.168.1.23]:54321",
			want: "<addr>",
		},
		{
			// 修复轮 3 真正要堵住的漏洞：zone id 带点号的 VLAN 子接口名。
			// round 2 的字符集 [0-9a-zA-Z:%] 不含 "."，这条会原样漏网；
			// 不枚举字符、只认"不是空白/不是 ]"之后，两端都能抹掉。
			name: "zone id 带点号的 VLAN 子接口名必须也被抹掉",
			in:   "[fe80::1%eth0.100]:54321",
			want: "<addr>",
		},
		{
			name: "超时错误必须原样通过",
			in:   "context deadline exceeded",
			want: "context deadline exceeded",
		},
		{
			name: "网络长时间无活动的超时必须原样通过",
			in:   "timeout: no recent network activity",
			want: "timeout: no recent network activity",
		},
		{
			name: "DNS 查询失败必须原样通过（主机名是操作者自己填的探针服务器地址，不是个人信息，且是有用的诊断信息）",
			in:   "lookup no-such-host.invalid: no such host",
			want: "lookup no-such-host.invalid: no such host",
		},
		{
			name: "另一条 DNS 查询失败也必须原样通过",
			in:   "lookup probe.example.com: no such host",
			want: "lookup probe.example.com: no such host",
		},
		{
			// 十六进制错误码不能被误当成地址抹掉——它不带点，IPv4 分支碰不到它，
			// 括号是圆括号不是方括号，方括号分支也碰不到它。
			name: "QUIC 错误码（十六进制）必须原样通过",
			in:   "CRYPTO_ERROR 0x12a (remote): x509: certificate signed by unknown authority",
			want: "CRYPTO_ERROR 0x12a (remote): x509: certificate signed by unknown authority",
		},
		{
			// 版本号 v0.48.2 只有两个点（三段），IPv4 分支要求恰好三个点（四段），
			// 不会误伤——这条性质不能破坏，否则版本号这条诊断信息就没了。
			name: "版本号必须原样通过（IPv4 需要四段三个点，版本号只有三段两个点）",
			in:   "INTERNAL_ERROR (local): quic-go v0.48.2 handshake failed",
			want: "INTERNAL_ERROR (local): quic-go v0.48.2 handshake failed",
		},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := scrubAddrs(tc.in); got != tc.want {
				t.Fatalf("scrubAddrs(%q) = %q, want %q", tc.in, got, tc.want)
			}
		})
	}
}

func TestScrubAddrsRemovesRealOpErrorAddresses(t *testing.T) {
	// 覆盖任务书里点名的那条真实形状，逐个断言三个具体值都不再出现，
	// 而不仅仅依赖上面表测试里的整串相等比较。
	got := scrubAddrs("dial udp 192.168.1.23:54321->203.0.113.7:4433: connect: no route to host")
	for _, leaked := range []string{"192.168.1.23", "203.0.113.7", "54321"} {
		if strings.Contains(got, leaked) {
			t.Fatalf("scrubAddrs leaked %q into %q", leaked, got)
		}
	}
}
