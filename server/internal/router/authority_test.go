package router

import (
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/control"
	"github.com/JianyueLab-Org/can-voice/server/internal/fsdfeed"
)

func TestControllerGrantRequiresSignedAndLiveAssignment(t *testing.T) {
	r := New()
	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{ByCallsign: map[string]fsdfeed.Position{
		"ZSPD_TWR": {CID: "1000", Callsign: "ZSPD_TWR", Facility: 4, FrequencyKHz: 118500, IsATC: true},
	}}})
	s := r.Add(SessionOpts{CID: "1000", Role: "controller", Callsign: "ZSPD_TWR", TXGrant: []uint32{118500}, GrantExpires: time.Now().Add(time.Minute), MaxTX: 2, MaxRX: 4})
	ack := r.Subscribe(s.ID, control.Sub{TX: []uint32{118500, 121800}, XC: [][2]uint32{{118500, 121800}}})
	if len(ack.TX) != 1 || ack.TX[0] != 118500 || len(ack.Rejected) != 1 || ack.Rejected[0] != 121800 || len(ack.RejectedXC) != 1 {
		t.Fatalf("unauthorized declaration accepted: %+v", ack)
	}
	if !r.MayTransmit(s.ID, 118500) || r.MayTransmit(s.ID, 121800) {
		t.Fatal("transmit did not honor the signed/live grant")
	}
	r.SetLocator(stubLocator{degraded: true})
	if r.MayTransmit(s.ID, 118500) {
		t.Fatal("feed loss did not stop transmit")
	}
}

func TestExpiredGrantKeepsReception(t *testing.T) {
	r := New()
	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{ByCID: map[string]fsdfeed.Position{
		"1000": {CID: "1000", Callsign: "CCA100", IsATC: false},
	}}})
	s := r.Add(SessionOpts{CID: "1000", Role: "pilot", GrantExpires: time.Now().Add(-time.Second), MaxTX: 1, MaxRX: 4})
	ack := r.Subscribe(s.ID, control.Sub{RX: []uint32{121800}, TX: []uint32{118500}})
	if len(ack.TX) != 0 || !contains(ack.RX, 121800) || r.MayTransmit(s.ID, 118500) {
		t.Fatalf("expired TX was admitted or RX lost: %+v", ack)
	}
}

func TestPilotCannotSubstituteAnotherFrequencyForSignedGrant(t *testing.T) {
	r := New()
	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{ByCID: map[string]fsdfeed.Position{
		"1000": {CID: "1000", Callsign: "CCA100"},
	}}})
	s := r.Add(SessionOpts{CID: "1000", Role: "pilot", TXGrant: []uint32{118500}, GrantExpires: time.Now().Add(time.Minute), MaxTX: 1, MaxRX: 4})
	ack := r.Subscribe(s.ID, control.Sub{TX: []uint32{121800}})
	if len(ack.TX) != 0 || len(ack.Rejected) != 1 || ack.Rejected[0] != 121800 || r.MayTransmit(s.ID, 121800) {
		t.Fatalf("unsigned pilot frequency was admitted: %+v", ack)
	}
	ack = r.Subscribe(s.ID, control.Sub{TX: []uint32{118500, 121800}})
	if len(ack.TX) != 1 || ack.TX[0] != 118500 || len(ack.Rejected) != 1 || ack.Rejected[0] != 121800 {
		t.Fatalf("pilot signed grant was not intersected with SUB: %+v", ack)
	}
}

func TestReconcileAuthorityRemovesTXAndXCButKeepsRX(t *testing.T) {
	r := New()
	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{ByCallsign: map[string]fsdfeed.Position{
		"ZSPD_TWR": {CID: "1000", Callsign: "ZSPD_TWR", Facility: 4, FrequencyKHz: 118500, IsATC: true},
	}}})
	s := r.Add(SessionOpts{CID: "1000", Role: "controller", Callsign: "ZSPD_TWR", TXGrant: []uint32{118500}, GrantExpires: time.Now().Add(time.Minute), MaxTX: 1, MaxRX: 4})
	r.Subscribe(s.ID, control.Sub{RX: []uint32{121800}, TX: []uint32{118500}})
	r.SetLocator(stubLocator{degraded: true})
	r.ReconcileAuthority()
	if got := s.subs.Load(); len(got.tx) != 0 || len(got.xc) != 0 || len(got.rx) != 2 {
		t.Fatalf("authority loss did not preserve reception and clear TX/XC: %+v", got)
	}
}

func TestGrantExpiryAutomaticallyClearsEffectiveTX(t *testing.T) {
	r := New()
	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{ByCID: map[string]fsdfeed.Position{
		"1000": {CID: "1000", Callsign: "CCA100"},
	}}})
	s := r.Add(SessionOpts{CID: "1000", Role: "pilot", TXGrant: []uint32{118500}, GrantExpires: time.Now().Add(50 * time.Millisecond), MaxTX: 1, MaxRX: 4})
	r.Subscribe(s.ID, control.Sub{TX: []uint32{118500}})
	if !r.MayTransmit(s.ID, 118500) {
		t.Fatal("fresh pilot grant was refused")
	}
	deadline := time.After(2 * time.Second)
	for len(s.subs.Load().tx) != 0 {
		select {
		case <-deadline:
			t.Fatal("expired grant remained in effective TX without a new SUB")
		case <-time.After(10 * time.Millisecond):
		}
	}
}

func TestObserverRequiresOwnFacilityZeroFSDSession(t *testing.T) {
	r := New()
	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{ByCID: map[string]fsdfeed.Position{
		"2000": {CID: "2000", Callsign: "OBS01", IsATC: true, IsObserver: true},
	}}})
	s := r.Add(SessionOpts{CID: "2000", Role: "observer", TXGrant: []uint32{122800}, GrantExpires: time.Now().Add(time.Minute), MaxTX: 2, MaxRX: 4})
	forged := r.Subscribe(s.ID, control.Sub{TX: []uint32{121800}})
	if len(forged.TX) != 0 || len(forged.Rejected) != 1 || forged.Rejected[0] != 121800 {
		t.Fatalf("observer substituted an unsigned frequency: %+v", forged)
	}
	ack := r.Subscribe(s.ID, control.Sub{TX: []uint32{122800, 121800}})
	if len(ack.TX) != 1 || ack.TX[0] != 122800 || len(ack.Rejected) != 1 {
		t.Fatalf("observer did not get exactly one in-band TX: %+v", ack)
	}
	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{ByCID: map[string]fsdfeed.Position{
		"2000": {CID: "2000", Callsign: "OBS01", IsATC: false},
	}}})
	if r.MayTransmit(s.ID, 122800) {
		t.Fatal("observer could transmit after FSD role changed to pilot")
	}
}

func TestControllerHandoffRevokesOldAssignment(t *testing.T) {
	r := New()
	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{ByCallsign: map[string]fsdfeed.Position{
		"ZSPD_TWR": {CID: "1000", Callsign: "ZSPD_TWR", IsATC: true, Facility: 4, FrequencyKHz: 118500},
	}}})
	s := r.Add(SessionOpts{CID: "1000", Role: "controller", Callsign: "ZSPD_TWR", TXGrant: []uint32{118500}, GrantExpires: time.Now().Add(time.Minute), MaxTX: 1, MaxRX: 4})
	r.Subscribe(s.ID, control.Sub{TX: []uint32{118500}})
	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{ByCallsign: map[string]fsdfeed.Position{
		"ZSPD_TWR": {CID: "3000", Callsign: "ZSPD_TWR", IsATC: true, Facility: 4, FrequencyKHz: 118500},
	}}})
	r.ReconcileAuthority()
	if r.MayTransmit(s.ID, 118500) || len(s.subs.Load().tx) != 0 {
		t.Fatal("handoff left the old controller with TX")
	}
}
