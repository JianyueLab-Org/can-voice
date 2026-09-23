package router

import (
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/fsdfeed"
)

const minVoiceKHz, maxVoiceKHz = 118000, 136975

// allowsTX intersects a signed scope with the current server-owned FSD state.
// A missing or degraded feed never authorizes transmission.
func (s *Session) allowsTX(freq uint32, snap fsdfeed.Snapshot, degraded bool, now time.Time) bool {
	if s.Role == "" { // Router-only compatibility; production handshakes use "legacy".
		return true
	}
	if degraded || s.GrantExpires.IsZero() || !now.Before(s.GrantExpires) {
		return false
	}
	switch s.Role {
	case "pilot", "observer":
		if _, ok := s.TXGrant[freq]; !ok {
			return false
		}
		if freq < minVoiceKHz || freq > maxVoiceKHz || freq%5 != 0 {
			return false
		}
		p, ok := snap.ByCID[s.CID]
		return ok && ((s.Role == "pilot" && !p.IsATC) || (s.Role == "observer" && p.IsObserver))
	case "controller":
		if _, ok := s.TXGrant[freq]; !ok {
			return false
		}
		p, ok := snap.ByCallsign[s.Callsign]
		return ok && p.CID == s.CID && p.IsATC && !p.IsATIS && !p.IsObserver && p.FrequencyKHz == freq
	case "atis":
		if _, ok := s.TXGrant[freq]; !ok {
			return false
		}
		p, ok := snap.ByCallsign[s.Station]
		// The ATIS voice account is intentionally separate from the FSD
		// publisher account.  The signed station/callsign and frequency grant
		// are the identity checks; requiring the CIDs to match rejects the
		// normal dedicated-ATIS deployment.
		return ok && p.Callsign == s.Station && p.IsATIS && p.FrequencyKHz == freq
	default:
		return false
	}
}

// ReconcileAuthority removes effective TX/XC after feed loss, assignment
// handoff, or ticket expiry. RX stays indexed until the next full SUB.
func (r *Router) ReconcileAuthority() {
	snap, degraded := r.positions()
	now := time.Now()
	var changed []*Session
	r.mu.Lock()
	for _, s := range r.sessions {
		old := s.subs.Load()
		if len(old.tx) == 0 {
			continue
		}
		allowed := true
		for f := range old.tx {
			if !s.allowsTX(f, snap, degraded, now) {
				allowed = false
				break
			}
		}
		if allowed {
			continue
		}
		r.bumpXC(old.xc, -1)
		next := &subs{rx: old.rx, tx: map[uint32]struct{}{}}
		s.subs.Store(next)
		changed = append(changed, s)
	}
	r.mu.Unlock()
	for _, s := range changed {
		go s.authorityLost()
	}
}
