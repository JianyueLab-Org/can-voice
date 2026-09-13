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
// 一起使用。client 和 connect 原本写在 Task 10 的端到端测试里，但 Task 9 的
// TestEvictionUsesTheEvictedCloseCode 也要用，所以提到这里——**Task 10 直接复用，
// 不要再写一份**，两份同名声明在同一个包里根本编译不过。

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
