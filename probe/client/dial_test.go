package main

import (
	"context"
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
