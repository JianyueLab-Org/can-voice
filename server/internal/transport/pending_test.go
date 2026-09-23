package transport

import (
	"context"
	"crypto/tls"
	"errors"
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/auth"
	"github.com/JianyueLab-Org/can-voice/server/internal/control"
	"github.com/JianyueLab-Org/can-voice/server/internal/router"
	"github.com/quic-go/quic-go"
)

func TestAdmissionGateBoundsPendingHandshakes(t *testing.T) {
	stats := &AdmissionStats{}
	gate := newAdmissionGate(2, stats)
	if !gate.tryAcquire() || !gate.tryAcquire() {
		t.Fatal("first two handshakes should be admitted")
	}
	if gate.tryAcquire() {
		t.Fatal("third handshake was admitted beyond the pending limit")
	}
	if got := stats.RejectedPending(); got != 1 {
		t.Fatalf("rejected pending = %d, want 1", got)
	}
	gate.release()
	if !gate.tryAcquire() {
		t.Fatal("a finished handshake did not free its admission slot")
	}
}

func TestAdmissionRejectsSecondConnectionBeforeAuthentication(t *testing.T) {
	cert, err := SelfSignedCert()
	if err != nil {
		t.Fatal(err)
	}
	stats := &AdmissionStats{}
	pub, priv := testKeys(t)
	cfg := Config{Addr: "127.0.0.1:0", TLS: &tls.Config{Certificates: []tls.Certificate{cert}}, PublicKey: pub, MaxRX: 4, MaxPendingHandshakes: 1, AdmissionStats: stats}
	ln, err := listen(cfg)
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(func() { cancel(); ln.Close() })
	gate := newAdmissionGate(1, stats)
	go accept(ctx, ln, cfg, router.New(), newConnSet(), gate)
	first := dial(t, ln.Addr().String())
	t.Cleanup(func() { first.CloseWithError(0, "") })
	deadline := time.After(3 * time.Second)
	for len(gate.slots) != 1 {
		select {
		case <-deadline:
			t.Fatal("first connection was not admitted")
		case <-time.After(10 * time.Millisecond):
		}
	}
	second := dial(t, ln.Addr().String())
	t.Cleanup(func() { second.CloseWithError(0, "") })
	select {
	case <-second.Context().Done():
	case <-time.After(3 * time.Second):
		t.Fatal("excess pending connection was not evicted")
	}
	var appErr *quic.ApplicationError
	if !errors.As(context.Cause(second.Context()), &appErr) || appErr.ErrorCode != CloseEvicted {
		t.Fatalf("second connection ended with %v, want CloseEvicted", context.Cause(second.Context()))
	}
	if got := stats.RejectedPending(); got != 1 {
		t.Fatalf("rejected pending = %d, want 1", got)
	}
	tok, err := auth.Sign(priv, auth.Claims{CID: "1000", Rating: 5, MaxTX: 1, Exp: time.Now().Add(time.Minute).Unix()})
	if err != nil {
		t.Fatal(err)
	}
	_, msg := hello(t, first, tok)
	if _, ok := msg.(*control.Ready); !ok {
		t.Fatalf("admitted handshake got %T, want READY", msg)
	}
	for len(gate.slots) != 0 {
		select {
		case <-time.After(3 * time.Second):
			t.Fatal("successful handshake did not release its slot")
		case <-time.After(10 * time.Millisecond):
		}
	}
}
