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
