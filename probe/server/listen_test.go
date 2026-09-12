package main

import (
	"crypto/tls"
	"testing"
)

func TestListenNegotiatesProbeALPN(t *testing.T) {
	tlsConf := selfSignedTLS(t)
	ln, err := Listen("127.0.0.1:0", tlsConf)
	if err != nil {
		t.Fatalf("Listen: %v", err)
	}
	defer ln.Close()

	if got := ln.Addr().String(); got == "" {
		t.Fatal("listener reported no address")
	}
	if ALPN != "can-voice-probe/1" {
		t.Fatalf("ALPN = %q, want can-voice-probe/1", ALPN)
	}
	if len(tlsConf.NextProtos) != 1 || tlsConf.NextProtos[0] != ALPN {
		t.Fatalf("Listen must pin NextProtos to %q, got %v", ALPN, tlsConf.NextProtos)
	}
}

func selfSignedTLS(t *testing.T) *tls.Config {
	t.Helper()
	cert, err := SelfSignedCert()
	if err != nil {
		t.Fatalf("SelfSignedCert: %v", err)
	}
	return &tls.Config{Certificates: []tls.Certificate{cert}}
}
