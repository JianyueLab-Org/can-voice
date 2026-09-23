package transport

import (
	"bytes"
	"crypto/ed25519"
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/auth"
	"github.com/JianyueLab-Org/can-voice/server/internal/control"
	"github.com/JianyueLab-Org/can-voice/server/internal/router"
)

func scopedStream(t *testing.T, priv ed25519.PrivateKey, claim auth.Claims, station, follow string) *memStream {
	t.Helper()
	tok, err := auth.Sign(priv, claim)
	if err != nil {
		t.Fatal(err)
	}
	b, err := control.Encode(&control.Hello{Token: tok, Client: "test/1", Proto: 1, Station: station, Follow: follow})
	if err != nil {
		t.Fatal(err)
	}
	var framed bytes.Buffer
	if err := control.WriteFrame(&framed, b); err != nil {
		t.Fatal(err)
	}
	return &memStream{in: bytes.NewReader(framed.Bytes())}
}

func TestHandshakeBindsOnlySignedATISStation(t *testing.T) {
	pub, priv := testKeys(t)
	cfg := Config{PublicKey: pub, MaxRX: 32}
	r := router.New()
	claim := auth.Claims{CID: "1000", Rating: 5, MaxTX: 1, Role: "atis", Station: "ZSPD_ATIS", TX: []int{118500}, Exp: time.Now().Add(time.Minute).Unix()}
	if _, err := handshake(scopedStream(t, priv, claim, "ZBAA_ATIS", ""), &stubConn{}, cfg, r, nil); err == nil {
		t.Fatal("HELLO was allowed to override the signed ATIS station")
	}
	s, err := handshake(scopedStream(t, priv, claim, "ZSPD_ATIS", ""), &stubConn{}, cfg, r, nil)
	if err != nil {
		t.Fatal(err)
	}
	if s.Station != "ZSPD_ATIS" || s.Role != "atis" {
		t.Fatalf("session scope = role %q, station %q", s.Role, s.Station)
	}
}

func TestHandshakeRejectsStationAndFollowForOrdinaryUsers(t *testing.T) {
	pub, priv := testKeys(t)
	cfg := Config{PublicKey: pub, MaxRX: 32}
	r := router.New()
	claim := auth.Claims{CID: "1000", Rating: 5, MaxTX: 8, Role: "pilot", Exp: time.Now().Add(time.Minute).Unix()}
	if _, err := handshake(scopedStream(t, priv, claim, "ZSPD_TWR", ""), &stubConn{}, cfg, r, nil); err == nil {
		t.Fatal("pilot supplied a station scope")
	}
	if _, err := handshake(scopedStream(t, priv, claim, "", "CCA101"), &stubConn{}, cfg, r, nil); err == nil {
		t.Fatal("pilot borrowed another FSD participant's position")
	}
	s, err := handshake(scopedStream(t, priv, claim, "", ""), &stubConn{}, cfg, r, nil)
	if err != nil {
		t.Fatal(err)
	}
	if s.Role != "pilot" || s.Station != "" || s.MaxTX != 1 {
		t.Fatalf("wrong session scope: %+v", s)
	}
}

func TestLegacyTicketIsReceptionOnly(t *testing.T) {
	pub, priv := testKeys(t)
	cfg := Config{PublicKey: pub, MaxRX: 32}
	r := router.New()
	claim := auth.Claims{CID: "1000", Rating: 5, MaxTX: 8, Exp: time.Now().Add(time.Minute).Unix()}
	s, err := handshake(scopedStream(t, priv, claim, "", ""), &stubConn{}, cfg, r, nil)
	if err != nil {
		t.Fatal(err)
	}
	if s.MaxTX != 0 || s.Role != "legacy" {
		t.Fatalf("legacy claim granted TX: role=%q max_tx=%d", s.Role, s.MaxTX)
	}
	ack := r.Subscribe(s.ID, control.Sub{RX: []uint32{121800}, TX: []uint32{118500}})
	if len(ack.TX) != 0 || len(ack.Rejected) != 1 || len(ack.RX) != 1 || ack.RX[0] != 121800 {
		t.Fatalf("legacy ticket is not RX-only: %+v", ack)
	}
}
