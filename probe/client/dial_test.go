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
			name: "超时错误必须原样通过",
			in:   "context deadline exceeded",
			want: "context deadline exceeded",
		},
		{
			name: "DNS 查询失败必须原样通过（主机名是操作者自己填的探针服务器地址，不是个人信息，且是有用的诊断信息）",
			in:   "lookup no-such-host.invalid: no such host",
			want: "lookup no-such-host.invalid: no such host",
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
