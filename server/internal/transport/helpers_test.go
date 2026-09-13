package transport

import (
	"crypto/ed25519"
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/auth"
	"github.com/JianyueLab-Org/can-voice/server/internal/control"
	"github.com/quic-go/quic-go"
)

// 本文件是 transport 包测试的共享脚手架，与 conn_test.go 的 testServer/dial/hello
// 一起使用。client、connect 和 subscribe 原本写在 Task 10 的端到端测试里，但 Task 9
// 这边也要用，所以提到这里——**Task 10 直接复用，不要再写一份**，两份同名声明在
// 同一个包里根本编译不过。

// client 是端到端测试用的最小客户端。
type client struct {
	conn quic.Connection
	st   quic.Stream
	sess uint32
}

func connect(t *testing.T, addr, cid string, maxTX int, priv ed25519.PrivateKey) *client {
	t.Helper()
	tok, err := auth.Sign(priv, auth.Claims{
		CID: cid, Rating: 5, MaxTX: maxTX, Exp: time.Now().Add(time.Minute).Unix(),
	})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	conn := dial(t, addr)
	st, m := hello(t, conn, tok)
	ready, ok := m.(*control.Ready)
	if !ok {
		t.Fatalf("handshake for %s got %T, want *control.Ready", cid, m)
	}
	return &client{conn: conn, st: st, sess: ready.Session}
}

// subscribe 发一份全量声明并**校验 SUBACK**。
//
// 校验这一步不是顺手加的。所有"听不见"的断言（频率隔离、耦合不生效）都有一个
// 共同的假冒通过方式：订阅其实被拒了，于是谁也没收到包，测试照样绿。把拒绝在
// 这里变成 Fatal，那一类假绿就没有了。
func (c *client) subscribe(t *testing.T, sub control.Sub) control.SubAck {
	t.Helper()
	b, err := control.Encode(&sub)
	if err != nil {
		t.Fatalf("Encode SUB: %v", err)
	}
	if err := control.WriteFrame(c.st, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	raw, err := control.ReadFrame(c.st)
	if err != nil {
		t.Fatalf("ReadFrame (SUBACK): %v", err)
	}
	m, err := control.Decode(raw)
	if err != nil {
		t.Fatalf("Decode SUBACK: %v", err)
	}
	ack, ok := m.(*control.SubAck)
	if !ok {
		t.Fatalf("after SUB got %T, want *control.SubAck", m)
	}
	if len(ack.Rejected) != 0 {
		t.Fatalf("SUB was partly rejected: %v — the test's premise is gone", ack.Rejected)
	}
	return *ack
}
