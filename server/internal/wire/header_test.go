package wire

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"os"
	"testing"
)

type goldenFile struct {
	HeaderSize int `json:"header_size"`
	Cases      []struct {
		Name   string `json:"name"`
		Header struct {
			Ver     uint8  `json:"ver"`
			Flags   uint8  `json:"flags"`
			Qual    uint8  `json:"qual"`
			Seq     uint16 `json:"seq"`
			FreqKHz uint32 `json:"freq_khz"`
			Speaker uint32 `json:"speaker"`
		} `json:"header"`
		OpusHex    string `json:"opus_hex"`
		EncodedHex string `json:"encoded_hex"`
	} `json:"cases"`
}

func loadGolden(t *testing.T) goldenFile {
	t.Helper()
	b, err := os.ReadFile("../../testdata/wire-golden.json")
	if err != nil {
		t.Fatalf("read golden: %v", err)
	}
	var g goldenFile
	if err := json.Unmarshal(b, &g); err != nil {
		t.Fatalf("parse golden: %v", err)
	}
	if len(g.Cases) == 0 {
		t.Fatal("golden file has no cases")
	}
	return g
}

func TestHeaderSizeMatchesGolden(t *testing.T) {
	if g := loadGolden(t); g.HeaderSize != HeaderSize {
		t.Fatalf("HeaderSize = %d, golden says %d", HeaderSize, g.HeaderSize)
	}
}

func TestAppendToMatchesGolden(t *testing.T) {
	for _, c := range loadGolden(t).Cases {
		t.Run(c.Name, func(t *testing.T) {
			opus, err := hex.DecodeString(c.OpusHex)
			if err != nil {
				t.Fatalf("bad opus_hex: %v", err)
			}
			h := Header{
				Ver: c.Header.Ver, Flags: c.Header.Flags, Qual: c.Header.Qual,
				Seq: c.Header.Seq, FreqKHz: c.Header.FreqKHz, Speaker: c.Header.Speaker,
			}
			got := hex.EncodeToString(append(h.AppendTo(nil), opus...))
			if got != c.EncodedHex {
				t.Fatalf("encoded = %s, golden = %s", got, c.EncodedHex)
			}
		})
	}
}

func TestParseMatchesGolden(t *testing.T) {
	for _, c := range loadGolden(t).Cases {
		t.Run(c.Name, func(t *testing.T) {
			raw, err := hex.DecodeString(c.EncodedHex)
			if err != nil {
				t.Fatalf("bad encoded_hex: %v", err)
			}
			h, opus, err := Parse(raw)
			if err != nil {
				t.Fatalf("Parse: %v", err)
			}
			if h.Ver != c.Header.Ver || h.Flags != c.Header.Flags || h.Qual != c.Header.Qual ||
				h.Seq != c.Header.Seq || h.FreqKHz != c.Header.FreqKHz || h.Speaker != c.Header.Speaker {
				t.Fatalf("header = %+v, golden = %+v", h, c.Header)
			}
			if hex.EncodeToString(opus) != c.OpusHex {
				t.Fatalf("opus = %s, golden = %s", hex.EncodeToString(opus), c.OpusHex)
			}
		})
	}
}

// TestTheHeaderConstantsAreTheLiteralValuesTheProtocolNames 钉住那几个**数字本身**。
//
// 和 transport 的 TestTheCloseCodesAndReasonsAreTheLiteralValuesTheProtocolNames
// 同一个形状、同一个理由：别的测试写的是 `h.Flags != wire.FlagLast`，那是拿常量
// 比常量——把 FlagLast 从 1<<1 改成 1<<2，router 那条 TestFanoutPreservesFlagLast
// 照样绿，因为比较的两边一起变了。而给它起名字的**全部理由**就是 Rust 侧会把
// 那一位写死在自己代码里：服务端改了、客户端没改，症状是 RX 指示灯在松开 PTT
// 之后一直亮着，而两边的测试各自还是绿的。
//
// HeaderSize 和 Version 另有金文件兜着（header_size 字段、每个样例的 ver），
// flags 没有：金文件里的 flags 是一个**数值**，1<<1 改成 1<<2 之后
// AppendTo/Parse 的字节流一个都没变，因为那个字节根本不是从常量来的。
func TestTheHeaderConstantsAreTheLiteralValuesTheProtocolNames(t *testing.T) {
	for _, tc := range []struct {
		name string
		got  uint8
		want uint8
	}{
		{"Version", Version, 1},
		{"FlagFirst", FlagFirst, 0x01},
		{"FlagLast", FlagLast, 0x02},
		{"ReservedFlags", ReservedFlags, 0xFC},
	} {
		if tc.got != tc.want {
			t.Errorf("%s = %#02x, want the wire value %#02x — the Rust client hardcodes this bit", tc.name, tc.got, tc.want)
		}
	}
	if HeaderSize != 13 {
		t.Errorf("HeaderSize = %d, want the wire value 13", HeaderSize)
	}
	// 三组位互不重叠、合起来正好一个字节。少了这两句，把 ReservedFlags 写成
	// 0xFE（连 FlagLast 一起"保留"）只会被上面那张表按数值抓到，
	// 抓不到"它和 FlagLast 撞了"这件事本身。
	if FlagFirst|FlagLast|ReservedFlags != 0xFF {
		t.Errorf("FlagFirst|FlagLast|ReservedFlags = %#02x, want 0xff — every bit of the flags byte must be either defined or reserved", FlagFirst|FlagLast|ReservedFlags)
	}
	if FlagFirst&FlagLast != 0 || (FlagFirst|FlagLast)&ReservedFlags != 0 {
		t.Errorf("the defined flag bits overlap the reserved ones: first=%#02x last=%#02x reserved=%#02x", FlagFirst, FlagLast, ReservedFlags)
	}
}

// TestParseAcceptsTheReservedFlagBits 钉住"保留位不是一个校验点"。
//
// 契约是"发送方置零、接收方忽略、服务端原样转发"（见 ReservedFlags 的注释）。
// 这条契约里最容易被下一个人好心改掉的就是它：给 Parse 加一句
// `if h.Flags&ReservedFlags != 0 { return error }` 看起来像在收紧协议，
// 实际是把将来任何一次"只升级客户端"的扩展变成一次全网 flag day。
func TestParseAcceptsTheReservedFlagBits(t *testing.T) {
	in := Header{Ver: 1, Flags: FlagFirst | ReservedFlags, FreqKHz: 121800}
	h, _, err := Parse(in.AppendTo(nil))
	if err != nil {
		t.Fatalf("Parse rejected a packet with the reserved flag bits set: %v — they are reserved-and-ignored, not reserved-and-refused", err)
	}
	if h.Flags != in.Flags {
		t.Fatalf("Flags = %#02x, want %#02x — Parse must not mask anything off; a silently cleared bit is the failure shape this protocol rejects everywhere else", h.Flags, in.Flags)
	}
}

func TestParseRejectsShortPacket(t *testing.T) {
	if _, _, err := Parse(make([]byte, HeaderSize-1)); err == nil {
		t.Fatal("Parse must reject a packet shorter than the header")
	}
}

func TestParseRejectsUnknownVersion(t *testing.T) {
	b := Header{Ver: 2, FreqKHz: 118000}.AppendTo(nil)
	if _, _, err := Parse(b); err == nil {
		t.Fatal("Parse must reject a version it does not know; silently accepting it would mean decoding a future layout as if it were this one")
	}
}

func TestParseAcceptsHeaderWithNoPayload(t *testing.T) {
	// 尾帧可以不带 Opus 载荷。
	b := Header{Ver: 1, Flags: FlagLast, FreqKHz: 118000}.AppendTo(nil)
	_, opus, err := Parse(b)
	if err != nil {
		t.Fatalf("Parse: %v", err)
	}
	if len(opus) != 0 {
		t.Fatalf("opus = %v, want empty", opus)
	}
}

// TestAppendToAppendsRatherThanOverwrites 钉住 AppendTo 的 dst 契约。
// 扇出路径会把包头追加进一个复用的缓冲区（那是零分配转发的做法），
// 所以"往非空 dst 里写"才是它真正的用法，而此前所有测试都只传 nil。
// 如果哪天它改成覆盖写，扇出出去的每个包都会少掉前缀、且只在 Task 8
// 那边表现为音频错乱——在这里钉住，坏了就当场红。
func TestAppendToAppendsRatherThanOverwrites(t *testing.T) {
	prefix := []byte{0xde, 0xad, 0xbe, 0xef}
	h := Header{Ver: 1, Flags: FlagFirst, Qual: 255, Seq: 7, FreqKHz: 121800, Speaker: 42}

	got := h.AppendTo(prefix)
	if len(got) != len(prefix)+HeaderSize {
		t.Fatalf("len = %d, want %d (prefix must survive)", len(got), len(prefix)+HeaderSize)
	}
	if !bytes.Equal(got[:len(prefix)], []byte{0xde, 0xad, 0xbe, 0xef}) {
		t.Fatalf("prefix was overwritten: %x", got[:len(prefix)])
	}
	if !bytes.Equal(got[len(prefix):], h.AppendTo(nil)) {
		t.Fatalf("header bytes differ when appended to a non-empty dst:\n got %x\nwant %x",
			got[len(prefix):], h.AppendTo(nil))
	}

	// 带富余容量的 dst：append 会就地写进那段容量，这是最容易写错成
	// "覆盖 dst 开头"的情形。
	spare := make([]byte, 4, 4+HeaderSize+8)
	copy(spare, prefix)
	got2 := h.AppendTo(spare)
	if !bytes.Equal(got2, got) {
		t.Fatalf("dst with spare capacity produced different bytes:\n got %x\nwant %x", got2, got)
	}
}
