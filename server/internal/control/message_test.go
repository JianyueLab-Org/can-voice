package control

import "testing"

func TestDecodeDispatchesOnTypeField(t *testing.T) {
	b := []byte(`{"type":"HELLO","token":"abc","client":"can-controller/3.0.0","proto":1}`)
	m, err := Decode(b)
	if err != nil {
		t.Fatalf("Decode: %v", err)
	}
	h, ok := m.(*Hello)
	if !ok {
		t.Fatalf("Decode returned %T, want *Hello", m)
	}
	if h.Token != "abc" || h.Proto != 1 {
		t.Fatalf("Hello = %+v", h)
	}
}

func TestDecodeRejectsUnknownType(t *testing.T) {
	if _, err := Decode([]byte(`{"type":"NOPE"}`)); err == nil {
		t.Fatal("Decode must reject an unknown message type")
	}
}

func TestSubCarriesFullDeclarationNotADelta(t *testing.T) {
	// SUB 是全量声明。它的字段名与结构不能暗示增量语义——
	// 没有 add/remove，只有完整的 rx/tx/xc 三张表。
	b := []byte(`{"type":"SUB","rx":[118000,121800],"tx":[121800],"xc":[[121800,124550]]}`)
	m, err := Decode(b)
	if err != nil {
		t.Fatalf("Decode: %v", err)
	}
	s := m.(*Sub)
	if len(s.RX) != 2 || s.RX[0] != 118000 || s.RX[1] != 121800 {
		t.Fatalf("RX = %v", s.RX)
	}
	if len(s.TX) != 1 || s.TX[0] != 121800 {
		t.Fatalf("TX = %v", s.TX)
	}
	if len(s.XC) != 1 || s.XC[0] != [2]uint32{121800, 124550} {
		t.Fatalf("XC = %v", s.XC)
	}
}

func TestEncodeRoundTripsThroughDecode(t *testing.T) {
	in := &Ready{Type: "READY", Session: 7, Server: "can-voice/1.0.0", MaxTX: 8, MaxRX: 32}
	b, err := Encode(in)
	if err != nil {
		t.Fatalf("Encode: %v", err)
	}
	m, err := Decode(b)
	if err != nil {
		t.Fatalf("Decode: %v", err)
	}
	out := m.(*Ready)
	if *out != *in {
		t.Fatalf("round trip: %+v != %+v", out, in)
	}
}

// Encode 对一个不认识的类型必须报错，而不是默默地按空 Type 序列化出去——
// 那样的帧在对端会被 Decode 当成未知类型拒绝，但发送方却毫不知情。
func TestEncodeRejectsUnknownType(t *testing.T) {
	if _, err := Encode(struct{ Type string }{Type: "NOPE"}); err == nil {
		t.Fatal("Encode must reject a type it does not know how to tag")
	}
}
