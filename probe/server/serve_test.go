package main

import (
	"context"
	"crypto/tls"
	"testing"
	"time"

	"github.com/quic-go/quic-go"
)

func TestServeEchoesDatagramsVerbatim(t *testing.T) {
	ln, err := Listen("127.0.0.1:0", &tls.Config{Certificates: []tls.Certificate{mustCert(t)}})
	if err != nil {
		t.Fatalf("Listen: %v", err)
	}
	defer ln.Close()
	go Serve(ln)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	conn, err := quic.DialAddr(ctx, ln.Addr().String(), &tls.Config{
		InsecureSkipVerify: true,
		NextProtos:         []string{ALPN},
	}, &quic.Config{EnableDatagrams: true})
	if err != nil {
		t.Fatalf("DialAddr: %v", err)
	}
	defer conn.CloseWithError(0, "")

	payload := []byte{0x01, 0x02, 0x03, 0xff, 0x00, 0x42}
	if err := conn.SendDatagram(payload); err != nil {
		t.Fatalf("SendDatagram: %v", err)
	}

	got, err := conn.ReceiveDatagram(ctx)
	if err != nil {
		t.Fatalf("ReceiveDatagram: %v", err)
	}
	if string(got) != string(payload) {
		t.Fatalf("echo = %v, want %v (must be verbatim)", got, payload)
	}
}

func mustCert(t *testing.T) tls.Certificate {
	t.Helper()
	cert, err := SelfSignedCert()
	if err != nil {
		t.Fatalf("SelfSignedCert: %v", err)
	}
	return cert
}
