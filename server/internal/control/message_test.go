package control

import (
	"encoding/json"
	"errors"
	"reflect"
	"testing"
)

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

// TestEncodeDecodeRoundTripsEveryMessageType 覆盖全部 8 种消息类型的编解码，
// 而不只是 HELLO/READY/SUB 这三种之前测过的。之前的覆盖缺口意味着
// SubAck/Notice/Ping/Pong/Bye 里任何一个 case 字符串或 json tag 写错，
// 这里都不会红——只会在真正连了一个客户端之后，表现成一条对不上号的
// "unknown control message type"。
//
// 每个用例的字段都填成非零值：一个全零的 Notice{} 即使 json tag 全错，
// 两边比较起来也仍然“相等”，因为零值处处相等，什么都测不出来。
func TestEncodeDecodeRoundTripsEveryMessageType(t *testing.T) {
	cases := []struct {
		name string
		msg  any
	}{
		{"Hello", &Hello{Token: "tok-9f3a", Client: "can-controller/3.0.0", Proto: 2, Follow: "CCA1501"}},
		{"Ready", &Ready{Session: 42, Server: "can-voice/1.0.0", MaxTX: 8, MaxRX: 32}},
		{"Sub", &Sub{RX: []uint32{118000, 121800}, TX: []uint32{121800}, XC: [][2]uint32{{121800, 124550}}}},
		{"SubAck", &SubAck{RX: []uint32{118000}, TX: []uint32{121800}, Rejected: []uint32{136975}}},
		{"Notice", &Notice{Kind: KindTxDenied, Freq: 121800, Reason: "no active radio track"}},
		{"Ping", &Ping{T: 1700000000}},
		{"Pong", &Pong{T: 1700000000, ServerT: 1700000042}},
		{"Bye", &Bye{Reason: "session superseded by a newer login"}},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			b, err := Encode(tc.msg)
			if err != nil {
				t.Fatalf("Encode: %v", err)
			}
			got, err := Decode(b)
			if err != nil {
				t.Fatalf("Decode: %v", err)
			}

			// 显式按具体类型断言，而不是只比较序列化后的字节——
			// 字节比较会漏掉“解成了另一个类型但恰好序列化相同”的情形。
			switch want := tc.msg.(type) {
			case *Hello:
				gotT, ok := got.(*Hello)
				if !ok {
					t.Fatalf("Decode returned %T, want *Hello", got)
				}
				if *gotT != *want {
					t.Fatalf("round trip: %+v != %+v", gotT, want)
				}
			case *Ready:
				gotT, ok := got.(*Ready)
				if !ok {
					t.Fatalf("Decode returned %T, want *Ready", got)
				}
				if *gotT != *want {
					t.Fatalf("round trip: %+v != %+v", gotT, want)
				}
			case *Sub:
				gotT, ok := got.(*Sub)
				if !ok {
					t.Fatalf("Decode returned %T, want *Sub", got)
				}
				if !reflect.DeepEqual(gotT, want) {
					t.Fatalf("round trip: %+v != %+v", gotT, want)
				}
			case *SubAck:
				gotT, ok := got.(*SubAck)
				if !ok {
					t.Fatalf("Decode returned %T, want *SubAck", got)
				}
				if !reflect.DeepEqual(gotT, want) {
					t.Fatalf("round trip: %+v != %+v", gotT, want)
				}
			case *Notice:
				gotT, ok := got.(*Notice)
				if !ok {
					t.Fatalf("Decode returned %T, want *Notice", got)
				}
				if *gotT != *want {
					t.Fatalf("round trip: %+v != %+v", gotT, want)
				}
			case *Ping:
				gotT, ok := got.(*Ping)
				if !ok {
					t.Fatalf("Decode returned %T, want *Ping", got)
				}
				if *gotT != *want {
					t.Fatalf("round trip: %+v != %+v", gotT, want)
				}
			case *Pong:
				gotT, ok := got.(*Pong)
				if !ok {
					t.Fatalf("Decode returned %T, want *Pong", got)
				}
				if *gotT != *want {
					t.Fatalf("round trip: %+v != %+v", gotT, want)
				}
			case *Bye:
				gotT, ok := got.(*Bye)
				if !ok {
					t.Fatalf("Decode returned %T, want *Bye", got)
				}
				if *gotT != *want {
					t.Fatalf("round trip: %+v != %+v", gotT, want)
				}
			default:
				t.Fatalf("unhandled case type %T in test table — add a branch above", tc.msg)
			}
		})
	}
}

// TestDecodeDistinguishesMalformedJSONFromUnknownType 确认（而不是改变）现状：
// Decode 对“格式错误的 JSON”和“格式正确但 type 未知”这两种失败，
// 返回的错误是可以区分的——前者包着一个 *json.SyntaxError，后者不包。
// 后续任务如果想把这两类日志分开记（一个是对端帧坏了，一个是版本比我们新），
// 可以靠 errors.As 解出 *json.SyntaxError 来分流，不需要再改这里的实现。
func TestDecodeDistinguishesMalformedJSONFromUnknownType(t *testing.T) {
	_, malformedErr := Decode([]byte(`{"type":`)) // 语法错误：value 缺失
	if malformedErr == nil {
		t.Fatal("Decode must reject malformed JSON")
	}
	var syntaxErr *json.SyntaxError
	if !errors.As(malformedErr, &syntaxErr) {
		t.Fatalf("malformed JSON error should unwrap to a *json.SyntaxError via errors.As, got %v", malformedErr)
	}

	_, unknownErr := Decode([]byte(`{"type":"NOPE"}`)) // 语法合法，type 未知
	if unknownErr == nil {
		t.Fatal("Decode must reject an unknown type")
	}
	var syntaxErr2 *json.SyntaxError
	if errors.As(unknownErr, &syntaxErr2) {
		t.Fatalf("an unknown-type error must not present as a JSON syntax error, got %v", unknownErr)
	}

	if malformedErr.Error() == unknownErr.Error() {
		t.Fatalf("the two error messages must not read identically: %q", malformedErr.Error())
	}
}
