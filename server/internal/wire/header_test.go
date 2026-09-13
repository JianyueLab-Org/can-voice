package wire

import (
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
