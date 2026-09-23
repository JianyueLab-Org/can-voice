package main

import (
	"encoding/base64"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/auth"
)

func TestFixtureTicketsGrantOnePilotFrequency(t *testing.T) {
	projectTemp := filepath.Join("..", "..", "..", ".temp")
	if err := os.MkdirAll(projectTemp, 0o755); err != nil {
		t.Fatal(err)
	}
	dir, err := os.MkdirTemp(projectTemp, "voice-e2e-fixture-test-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := os.RemoveAll(dir); err != nil {
			t.Error(err)
		}
	})
	writeTokens(dir)
	encodedPub, err := os.ReadFile(filepath.Join(dir, "api.pub"))
	if err != nil {
		t.Fatal(err)
	}
	pub, err := base64.StdEncoding.DecodeString(string(encodedPub))
	if err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct{ file, cid string }{
		{"token.txt", "1000"}, {"token-b.txt", "1001"},
	} {
		t.Run(tc.file, func(t *testing.T) {
			b, err := os.ReadFile(filepath.Join(dir, tc.file))
			if err != nil {
				t.Fatal(err)
			}
			claim, err := auth.Verify(pub, string(b), time.Now())
			if err != nil {
				t.Fatal(err)
			}
			if claim.CID != tc.cid || claim.Role != "pilot" || claim.MaxTX != 1 || !reflect.DeepEqual(claim.TX, []int{121800}) {
				t.Fatalf("fixture claim = %+v", claim)
			}
		})
	}
	expired, err := os.ReadFile(filepath.Join(dir, "token-expired.txt"))
	if err != nil {
		t.Fatal(err)
	}
	if _, err := auth.Verify(pub, string(expired), time.Now()); !errors.Is(err, auth.ErrExpired) {
		t.Fatalf("expired fixture verification = %v, want ErrExpired", err)
	}
}
