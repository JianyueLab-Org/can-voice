package transport

import (
	"bytes"
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/control"
	"github.com/JianyueLab-Org/can-voice/server/internal/fsdfeed"
	"github.com/JianyueLab-Org/can-voice/server/internal/router"
)

type authorityLocator struct {
	snap     fsdfeed.Snapshot
	degraded bool
}

func (l authorityLocator) Positions() (fsdfeed.Snapshot, bool) { return l.snap, l.degraded }

func TestAuthorityLossAfterNewerSubAckStillReleasesPTT(t *testing.T) {
	r := router.New()
	live := authorityLocator{snap: fsdfeed.Snapshot{ByCallsign: map[string]fsdfeed.Position{
		"ZSPD_TWR": {CID: "1000", Callsign: "ZSPD_TWR", IsATC: true, FrequencyKHz: 118500},
	}}}
	r.SetLocator(live)
	s := r.Add(router.SessionOpts{CID: "1000", Role: "controller", Callsign: "ZSPD_TWR", TXGrant: []uint32{118500}, GrantExpires: time.Now().Add(time.Minute), MaxTX: 1, MaxRX: 1})
	st := &memStream{}
	cw := &controlWriter{st: st}
	r.Subscribe(s.ID, control.Sub{TX: []uint32{118500}})
	r.SetLocator(authorityLocator{degraded: true})
	r.ReconcileAuthority()
	staleRevision := s.SubscriptionRevision()
	if _, err := cw.subscribe(r, s, control.Sub{RX: []uint32{121800}}); err != nil {
		t.Fatal(err)
	}
	if err := cw.writeAuthorityNotice(s, control.KindAuthorityLost, staleRevision); err != nil {
		t.Fatal(err)
	}
	reader := bytes.NewReader(st.out.Bytes())
	if msg := readControlMessage(t, reader); subAckRX(t, msg) != 121800 {
		t.Fatalf("first frame = %+v, want newer SUBACK", msg)
	}
	notice, ok := readControlMessage(t, reader).(*control.Notice)
	if !ok || notice.Kind != control.KindAuthorityLost {
		t.Fatalf("second frame = %+v, want loss notice requiring replay", notice)
	}
	if reader.Len() != 0 {
		t.Fatal("unexpected extra control frame")
	}
}

func TestReversedLossAndRestoreCallbacksStillNotifyLossFirst(t *testing.T) {
	r := router.New()
	live := authorityLocator{snap: fsdfeed.Snapshot{ByCallsign: map[string]fsdfeed.Position{
		"ZSPD_TWR": {CID: "1000", Callsign: "ZSPD_TWR", IsATC: true, FrequencyKHz: 118500},
	}}}
	r.SetLocator(live)
	s := r.Add(router.SessionOpts{CID: "1000", Role: "controller", Callsign: "ZSPD_TWR", TXGrant: []uint32{118500}, GrantExpires: time.Now().Add(time.Minute), MaxTX: 1, MaxRX: 1})
	r.Subscribe(s.ID, control.Sub{TX: []uint32{118500}})
	r.SetLocator(authorityLocator{degraded: true})
	r.ReconcileAuthority()
	lossRevision := s.SubscriptionRevision()
	r.SetLocator(live)
	r.ReconcileAuthority()
	restoreRevision := s.SubscriptionRevision()
	st := &memStream{}
	cw := &controlWriter{st: st}
	if err := cw.writeAuthorityNotice(s, control.KindAuthorityRestored, restoreRevision); err != nil {
		t.Fatal(err)
	}
	if err := cw.writeAuthorityNotice(s, control.KindAuthorityLost, lossRevision); err != nil {
		t.Fatal(err)
	}
	reader := bytes.NewReader(st.out.Bytes())
	first, ok := readControlMessage(t, reader).(*control.Notice)
	if !ok || first.Kind != control.KindAuthorityLost {
		t.Fatalf("first notice = %+v, want loss", first)
	}
	second, ok := readControlMessage(t, reader).(*control.Notice)
	if !ok || second.Kind != control.KindAuthorityRestored {
		t.Fatalf("second notice = %+v, want restore", second)
	}
	if reader.Len() != 0 {
		t.Fatal("duplicate notice after delayed loss callback")
	}
}

func TestAuthorityNoticeBeforeSubAckKeepsWireOrder(t *testing.T) {
	r := router.New()
	live := authorityLocator{snap: fsdfeed.Snapshot{ByCallsign: map[string]fsdfeed.Position{
		"ZSPD_TWR": {CID: "1000", Callsign: "ZSPD_TWR", IsATC: true, FrequencyKHz: 118500},
	}}}
	r.SetLocator(live)
	s := r.Add(router.SessionOpts{CID: "1000", Role: "controller", Callsign: "ZSPD_TWR", TXGrant: []uint32{118500}, GrantExpires: time.Now().Add(time.Minute), MaxTX: 1, MaxRX: 1})
	st := &memStream{}
	cw := &controlWriter{st: st}
	r.Subscribe(s.ID, control.Sub{TX: []uint32{118500}})
	r.SetLocator(authorityLocator{degraded: true})
	r.ReconcileAuthority()
	if err := cw.writeAuthorityNotice(s, control.KindAuthorityLost, s.SubscriptionRevision()); err != nil {
		t.Fatal(err)
	}
	if _, err := cw.subscribe(r, s, control.Sub{RX: []uint32{121800}}); err != nil {
		t.Fatal(err)
	}
	reader := bytes.NewReader(st.out.Bytes())
	notice, ok := readControlMessage(t, reader).(*control.Notice)
	if !ok || notice.Kind != control.KindAuthorityLost {
		t.Fatalf("first frame = %+v, want authority loss", notice)
	}
	if msg := readControlMessage(t, reader); subAckRX(t, msg) != 121800 {
		t.Fatalf("second frame = %+v, want SUBACK", msg)
	}
}

func TestRestoreNoticeSurvivesInFlightSubAck(t *testing.T) {
	r := router.New()
	live := authorityLocator{snap: fsdfeed.Snapshot{ByCallsign: map[string]fsdfeed.Position{
		"ZSPD_TWR": {CID: "1000", Callsign: "ZSPD_TWR", IsATC: true, FrequencyKHz: 118500},
	}}}
	r.SetLocator(live)
	s := r.Add(router.SessionOpts{CID: "1000", Role: "controller", Callsign: "ZSPD_TWR", TXGrant: []uint32{118500}, GrantExpires: time.Now().Add(time.Minute), MaxTX: 1, MaxRX: 2})
	st := &memStream{}
	cw := &controlWriter{st: st}
	r.Subscribe(s.ID, control.Sub{TX: []uint32{118500}})
	r.SetLocator(authorityLocator{degraded: true})
	r.ReconcileAuthority()
	ackDone := make(chan error, 1)
	noticeDone := make(chan error, 1)
	s.SetNotifyAuthorityChange(func(kind string, revision uint64) {
		noticeDone <- cw.writeAuthorityNotice(s, kind, revision)
	})
	cw.mu.Lock()
	r.SetLocator(live)
	r.ReconcileAuthority()
	restoredRevision := s.SubscriptionRevision()
	go func() {
		_, err := cw.subscribe(r, s, control.Sub{RX: []uint32{121800}, TX: []uint32{118500}})
		ackDone <- err
	}()
	cw.mu.Unlock()
	if err := <-ackDone; err != nil {
		t.Fatal(err)
	}
	if err := <-noticeDone; err != nil {
		t.Fatal(err)
	}
	reader := bytes.NewReader(st.out.Bytes())
	var kinds []string
	for reader.Len() != 0 {
		switch message := readControlMessage(t, reader).(type) {
		case *control.Notice:
			kinds = append(kinds, message.Kind)
		case *control.SubAck:
			if len(message.TX) != 1 || message.TX[0] != 118500 {
				t.Fatalf("in-flight SUBACK did not grant requested TX: %+v", message)
			}
			kinds = append(kinds, "suback")
		default:
			t.Fatalf("unexpected control message: %+v", message)
		}
	}
	if len(kinds) < 2 || kinds[0] == control.KindAuthorityRestored || kinds[len(kinds)-1] == control.KindAuthorityRestored {
		t.Fatalf("loss must precede restore and stale restore must not follow SUBACK: %v", kinds)
	}
	if kinds[0] == "suback" {
		if len(kinds) != 2 || kinds[1] != control.KindAuthorityLost {
			t.Fatalf("late callback order = %v, want SUBACK then loss", kinds)
		}
	} else if kinds[0] != control.KindAuthorityLost ||
		!(len(kinds) == 2 && kinds[1] == "suback" ||
			len(kinds) == 3 && kinds[1] == control.KindAuthorityRestored && kinds[2] == "suback") {
		t.Fatalf("early callback order = %v", kinds)
	}
	if s.SubscriptionRevision() <= restoredRevision {
		t.Fatal("in-flight SUB did not advance authority revision")
	}
}

func readControlMessage(t *testing.T, reader *bytes.Reader) any {
	t.Helper()
	raw, err := control.ReadFrame(reader)
	if err != nil {
		t.Fatal(err)
	}
	message, err := control.Decode(raw)
	if err != nil {
		t.Fatal(err)
	}
	return message
}

func subAckRX(t *testing.T, message any) uint32 {
	t.Helper()
	ack, ok := message.(*control.SubAck)
	if !ok || len(ack.RX) != 1 {
		t.Fatalf("message = %+v, want one-frequency SUBACK", message)
	}
	return ack.RX[0]
}
