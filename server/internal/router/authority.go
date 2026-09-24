package router

import (
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/control"
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
	for {
		snap, degraded, epoch := r.positionsWithEpoch()
		now := time.Now()
		type change struct {
			session  *Session
			kind     string
			revision uint64
		}
		var changed []change
		r.mu.Lock()
		if r.locatorEpoch != epoch {
			r.mu.Unlock()
			continue
		}
		for _, s := range r.sessions {
			old := s.subs.Load()
			if len(old.tx) != 0 {
				allowed := true
				for f := range old.tx {
					if !s.allowsTX(f, snap, degraded, now) {
						allowed = false
						break
					}
				}
				if !allowed {
					r.bumpXC(old.xc, -1)
					s.subs.Store(&subs{rx: old.rx, tx: map[uint32]struct{}{}, requestedTX: old.requestedTX, replayPending: len(old.requestedTX) > 0})
					revision := s.subRevision.Add(1)
					s.lastLossRevision.Store(revision)
					changed = append(changed, change{s, control.KindAuthorityLost, revision})
				}
				continue
			}
			if !old.replayPending {
				continue
			}
			eligible := false
			for f := range old.requestedTX {
				if s.allowsTX(f, snap, degraded, now) {
					eligible = true
					break
				}
			}
			if eligible && !old.restoreNotified {
				s.subs.Store(&subs{rx: old.rx, tx: old.tx, requestedTX: old.requestedTX, replayPending: true, restoreNotified: true})
				changed = append(changed, change{s, control.KindAuthorityRestored, s.subRevision.Add(1)})
			} else if !eligible && old.restoreNotified {
				s.subs.Store(&subs{rx: old.rx, tx: old.tx, requestedTX: old.requestedTX, replayPending: true})
				s.subRevision.Add(1)
			}
		}
		r.mu.Unlock()
		for _, c := range changed {
			go c.session.authorityChange(c.kind, c.revision)
		}
		return
	}
}
