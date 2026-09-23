package transport

import "sync/atomic"

const defaultMaxPendingHandshakes = 128

// AdmissionStats exposes only an aggregate count, never peer identifiers.
type AdmissionStats struct{ rejected atomic.Uint64 }

func (s *AdmissionStats) RejectedPending() uint64 { return s.rejected.Load() }

type admissionGate struct {
	slots chan struct{}
	stats *AdmissionStats
}

func newAdmissionGate(limit int, stats *AdmissionStats) *admissionGate {
	if limit <= 0 {
		limit = defaultMaxPendingHandshakes
	}
	if stats == nil {
		stats = &AdmissionStats{}
	}
	return &admissionGate{slots: make(chan struct{}, limit), stats: stats}
}

func (g *admissionGate) tryAcquire() bool {
	select {
	case g.slots <- struct{}{}:
		return true
	default:
		g.stats.rejected.Add(1)
		return false
	}
}

func (g *admissionGate) release() { <-g.slots }
