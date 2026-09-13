package router

import (
	"testing"

	"github.com/JianyueLab-Org/can-voice/server/internal/control"
	"github.com/JianyueLab-Org/can-voice/server/internal/fsdfeed"
	"github.com/JianyueLab-Org/can-voice/server/internal/wire"
)

// stubLocator 让射程测试不必碰网络。
type stubLocator struct {
	snap     fsdfeed.Snapshot
	degraded bool
}

func (s stubLocator) Snapshot() fsdfeed.Snapshot { return s.snap }
func (s stubLocator) Degraded() bool             { return s.degraded }

// recorder 是一个记录收到了什么的会话。
type recorder struct {
	sess *Session
	got  [][]byte
}

func newRecorder(t *testing.T, r *Router, cid string) *recorder {
	t.Helper()
	rec := &recorder{}
	rec.sess = r.Add(SessionOpts{
		CID: cid, MaxTX: 8, MaxRX: 64,
		Send: func(b []byte) { rec.got = append(rec.got, append([]byte(nil), b...)) },
	})
	return rec
}

// airborne 造一个已知位置的飞行员。losTerm 直接给，免得测试去算平方根。
func airborne(callsign, cid string, lat, lon, losTerm float64) fsdfeed.Position {
	return fsdfeed.Position{
		Callsign: callsign, CID: cid, Known: true,
		Lat: lat, Lon: lon, LOSTermNM: losTerm,
	}
}

// atc 造一个已知位置的管制席位。
func atc(callsign, cid string, lat, lon, radius float64) fsdfeed.Position {
	return fsdfeed.Position{
		Callsign: callsign, CID: cid, Known: true,
		Lat: lat, Lon: lon, RadiusNM: radius, IsATC: true,
	}
}

func packet(freq uint32, seq uint16, opus ...byte) []byte {
	return append(wire.Header{
		Ver: wire.Version, Flags: wire.FlagFirst, Seq: seq, FreqKHz: freq,
	}.AppendTo(nil), opus...)
}

func TestFanoutDeliversToListenersButNotBackToTheSender(t *testing.T) {
	r := New()
	speaker := newRecorder(t, r, "1000")
	listener := newRecorder(t, r, "1001")
	r.Subscribe(speaker.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(listener.sess.ID, control.Sub{RX: []uint32{121800}})

	n, err := r.Fanout(speaker.sess.ID, packet(121800, 1, 0xAA))
	if err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if n != 1 {
		t.Fatalf("delivered = %d, want 1", n)
	}
	if len(listener.got) != 1 {
		t.Fatalf("listener got %d packets, want 1", len(listener.got))
	}
	if len(speaker.got) != 0 {
		t.Fatal("the speaker must not hear themselves — TX implies RX, so they are their own listener and must be skipped explicitly")
	}
}

func TestFanoutStampsSpeakerAndLeavesOpusUntouched(t *testing.T) {
	r := New()
	speaker := newRecorder(t, r, "1000")
	listener := newRecorder(t, r, "1001")
	r.Subscribe(speaker.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(listener.sess.ID, control.Sub{RX: []uint32{121800}})

	opus := []byte{0x01, 0x02, 0x03, 0x04}
	if _, err := r.Fanout(speaker.sess.ID, packet(121800, 7, opus...)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	h, payload, err := wire.Parse(listener.got[0])
	if err != nil {
		t.Fatalf("Parse: %v", err)
	}
	if h.Speaker != uint32(speaker.sess.ID) {
		t.Fatalf("Speaker = %d, want %d", h.Speaker, speaker.sess.ID)
	}
	if h.Seq != 7 || h.Flags != wire.FlagFirst || h.FreqKHz != 121800 {
		t.Fatalf("header = %+v, want seq 7 / FlagFirst / 121800 preserved", h)
	}
	if string(payload) != string(opus) {
		t.Fatalf("opus = %v, want %v — the server must never touch the audio", payload, opus)
	}
}

func TestFanoutRefusesAnUndeclaredTransmitFrequency(t *testing.T) {
	r := New()
	speaker := newRecorder(t, r, "1000")
	listener := newRecorder(t, r, "1001")
	r.Subscribe(speaker.sess.ID, control.Sub{RX: []uint32{121800}}) // 只收不发
	r.Subscribe(listener.sess.ID, control.Sub{RX: []uint32{121800}})

	if _, err := r.Fanout(speaker.sess.ID, packet(121800, 1)); err == nil {
		t.Fatal("a session that did not declare TX must not be able to transmit; otherwise anyone can shout on any frequency")
	}
	if len(listener.got) != 0 {
		t.Fatal("a refused packet must not reach anyone")
	}
}

// --- 射程 ---

// TestALowAircraftReachesTheWholeControllerRadius 是原本那条合并规则最恶劣的后果。
//
// 那条规则取 min(发送方射程, 听众射程)。飞行员那一侧存的是视距**半项**，低空时很小：
// 1000 英尺是 1.23×√1000 ≈ 39 海里。一架刚起飞的飞机在 60 海里外呼叫进近，
// min(39, 80) = 39 → 比值 1.54 → 判"不扇出"。**他叫不到进近。**
// 正确的答案是进近声明的 80 海里说了算，比值 0.75，满格。
//
// 注意这个测试的数字是挑过的：它必须在两条规则下给出**不同**的结果。
// "停在机坪的飞机呼叫塔台"不行——Task 6B 给地面飞机加了 20 英尺的天线高度下限
// （半项 5.5 海里），而塔台就在 0.5 海里外，两条规则都会放行，那个测试无论实现
// 对错都是绿的。一个 bug 的修复消解了另一个 bug 的测试。
func TestALowAircraftReachesTheWholeControllerRadius(t *testing.T) {
	r := New()
	pilot := newRecorder(t, r, "1000")
	app := newRecorder(t, r, "1001")
	r.Subscribe(pilot.sess.ID, control.Sub{TX: []uint32{119200}})
	r.Subscribe(app.sess.ID, control.Sub{RX: []uint32{119200}})

	// 飞机在 1000 英尺，半项约 39 海里；进近在 60 海里外，半径 80。
	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{ByCID: map[string]fsdfeed.Position{
		"1000": airborne("CCA1", "1000", 30.0, 120.0, 38.9),
		"1001": atc("ZSPD_APP", "1001", 31.0, 120.0, 80),
	}}})

	n, err := r.Fanout(pilot.sess.ID, packet(119200, 1, 0xAA))
	if err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if n != 1 || len(app.got) != 1 {
		t.Fatalf("delivered = %d, approach got %d — a departing aircraft at 1000 ft must reach approach 60 NM away; min() of the two would give 39 NM and cut it off", n, len(app.got))
	}
	h, _, _ := wire.Parse(app.got[0])
	if h.Qual != 255 {
		t.Fatalf("Qual = %d, want 255 — 60 NM is inside 0.8 × 80", h.Qual)
	}
}

// TestAHighAircraftReachesTheWholeControllerRadius 是同一条规则的另一个方向。
// min 会把 CTR 的权威 600 海里砍成飞行员的半项 230。
func TestAHighAircraftReachesTheWholeControllerRadius(t *testing.T) {
	r := New()
	pilot := newRecorder(t, r, "1000")
	ctr := newRecorder(t, r, "1001")
	r.Subscribe(pilot.sess.ID, control.Sub{TX: []uint32{124550}})
	r.Subscribe(ctr.sess.ID, control.Sub{RX: []uint32{124550}})

	// 相距约 400 海里：在 CTR 的 600 海里内，但超过飞行员 230 海里的半项。
	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{ByCID: map[string]fsdfeed.Position{
		"1000": airborne("CCA1", "1000", 30.0, 120.0, 230),
		"1001": atc("ZSHA_CTR", "1001", 36.67, 120.0, 600),
	}}})

	n, err := r.Fanout(pilot.sess.ID, packet(124550, 1, 0xAA))
	if err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if n != 1 {
		t.Fatalf("delivered = %d, want 1 — the controller's declared radius governs, not the pilot's line-of-sight half-term", n)
	}
}

// TestTwoAirborneAircraftSumTheirHalfTerms 是第三种组合。
// 取 max 会把两架 FL350 的 460 海里砍成 230。
func TestTwoAirborneAircraftSumTheirHalfTerms(t *testing.T) {
	r := New()
	a := newRecorder(t, r, "1000")
	b := newRecorder(t, r, "1001")
	r.Subscribe(a.sess.ID, control.Sub{TX: []uint32{123450}})
	r.Subscribe(b.sess.ID, control.Sub{RX: []uint32{123450}})

	// 相距约 300 海里：小于 230+230=460 的 0.8 倍（368），所以应当满格。
	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{ByCID: map[string]fsdfeed.Position{
		"1000": airborne("CCA1", "1000", 30.0, 120.0, 230),
		"1001": airborne("CCA2", "1001", 35.0, 120.0, 230),
	}}})

	if _, err := r.Fanout(a.sess.ID, packet(123450, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(b.got) != 1 {
		t.Fatal("two FL350 aircraft 300 NM apart must hear each other; max() of their half-terms would give 230 and cut them off")
	}
	h, _, _ := wire.Parse(b.got[0])
	if h.Qual != 255 {
		t.Fatalf("Qual = %d, want 255 — 300 NM is well inside 0.8 × 460", h.Qual)
	}
}

func TestFanoutSkipsListenersOutOfRange(t *testing.T) {
	r := New()
	a := newRecorder(t, r, "1000")
	b := newRecorder(t, r, "1001")
	r.Subscribe(a.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(b.sess.ID, control.Sub{RX: []uint32{121800}})

	// 相距约 480 海里，射程合计 40 海里。
	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{ByCID: map[string]fsdfeed.Position{
		"1000": airborne("CCA1", "1000", 30.0, 120.0, 20),
		"1001": airborne("CCA2", "1001", 38.0, 120.0, 20),
	}}})

	n, err := r.Fanout(a.sess.ID, packet(121800, 1, 0xAA))
	if err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if n != 0 || len(b.got) != 0 {
		t.Fatalf("delivered = %d, listener got %d — 480 NM apart with 40 NM of range must not be forwarded", n, len(b.got))
	}
}

func TestFanoutStampsQualityForListenersInTheEdgeBand(t *testing.T) {
	r := New()
	a := newRecorder(t, r, "1000")
	near := newRecorder(t, r, "1001")
	edge := newRecorder(t, r, "1002")
	r.Subscribe(a.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(near.sess.ID, control.Sub{RX: []uint32{121800}})
	r.Subscribe(edge.sess.ID, control.Sub{RX: []uint32{121800}})

	// 射程合计 200 海里。满格到 160，160–220 之间线性衰减。
	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{ByCID: map[string]fsdfeed.Position{
		"1000": airborne("CCA1", "1000", 30.0, 120.0, 100),
		"1001": airborne("CCA2", "1001", 30.5, 120.0, 100), // 约 30 海里
		"1002": airborne("CCA3", "1002", 33.0, 120.0, 100), // 约 180 海里
	}}})

	if _, err := r.Fanout(a.sess.ID, packet(121800, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	hn, _, _ := wire.Parse(near.got[0])
	if hn.Qual != 255 {
		t.Fatalf("near Qual = %d, want 255", hn.Qual)
	}
	he, _, _ := wire.Parse(edge.got[0])
	if he.Qual == 0 || he.Qual == 255 {
		t.Fatalf("edge Qual = %d, want something strictly between 0 and 255 — 180 NM is inside the 160–220 taper", he.Qual)
	}
}

func TestFanoutDeliversToEveryoneWhenTheFeedIsDegraded(t *testing.T) {
	r := New()
	a := newRecorder(t, r, "1000")
	b := newRecorder(t, r, "1001")
	r.Subscribe(a.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(b.sess.ID, control.Sub{RX: []uint32{121800}})
	r.SetLocator(stubLocator{degraded: true})

	if _, err := r.Fanout(a.sess.ID, packet(121800, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(b.got) != 1 {
		t.Fatal("when we do not know where anyone is we must pass everything, not block everything")
	}
}

func TestFanoutDeliversWithNoLocatorAtAll(t *testing.T) {
	// 没装 Locator 等于永久降级。启动顺序里 Fanout 完全可能先于 SetLocator。
	r := New()
	a := newRecorder(t, r, "1000")
	b := newRecorder(t, r, "1001")
	r.Subscribe(a.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(b.sess.ID, control.Sub{RX: []uint32{121800}})

	if _, err := r.Fanout(a.sess.ID, packet(121800, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(b.got) != 1 {
		t.Fatal("with no locator installed every packet must be forwarded")
	}
}

func TestFanoutDeliversWhenAPositionIsUnknown(t *testing.T) {
	r := New()
	a := newRecorder(t, r, "1000")
	absent := newRecorder(t, r, "1001")  // 完全不在快照里
	unknown := newRecorder(t, r, "1002") // 在快照里但没有坐标
	r.Subscribe(a.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(absent.sess.ID, control.Sub{RX: []uint32{121800}})
	r.Subscribe(unknown.sess.ID, control.Sub{RX: []uint32{121800}})

	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{ByCID: map[string]fsdfeed.Position{
		"1000": airborne("CCA1", "1000", 30.0, 120.0, 100),
		"1002": {Callsign: "CCA3", CID: "1002", Known: false},
	}}})

	if _, err := r.Fanout(a.sess.ID, packet(121800, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(absent.got) != 1 {
		t.Fatal("someone who connected voice but not FSD must not be struck deaf by it")
	}
	if len(unknown.got) != 1 {
		t.Fatal("a participant whose position can-fsd does not know yet must not be struck deaf either; that is an aircraft on the apron about to call for clearance")
	}
}

// TestAnUnknownSpeakerPositionDisablesFilteringEntirely 是上一条的对称面。
func TestAnUnknownSpeakerPositionDisablesFilteringEntirely(t *testing.T) {
	r := New()
	a := newRecorder(t, r, "1000")
	far := newRecorder(t, r, "1001")
	r.Subscribe(a.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(far.sess.ID, control.Sub{RX: []uint32{121800}})

	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{ByCID: map[string]fsdfeed.Position{
		"1001": airborne("CCA2", "1001", 60.0, 10.0, 20), // 半个地球外
	}}})

	if _, err := r.Fanout(a.sess.ID, packet(121800, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(far.got) != 1 {
		t.Fatal("if we do not know where the speaker is we cannot filter at all; pass rather than block")
	}
}

// TestObserverModeUsesTheFollowedAircraftsPosition 覆盖观察员。
func TestObserverModeUsesTheFollowedAircraftsPosition(t *testing.T) {
	r := New()
	speaker := newRecorder(t, r, "1000")
	obs := &recorder{}
	obs.sess = r.Add(SessionOpts{
		CID: "9999", Follow: "CCA1", MaxTX: 0, MaxRX: 64,
		Send: func(b []byte) { obs.got = append(obs.got, append([]byte(nil), b...)) },
	})
	r.Subscribe(speaker.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(obs.sess.ID, control.Sub{RX: []uint32{121800}})

	// 观察员自己没有 FSD 连接，所以 ByCID 里没有 9999；他的位置来自
	// 他跟随的那架飞机，而那架飞机就在说话人旁边。
	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{
		ByCID: map[string]fsdfeed.Position{
			"1000": airborne("CCA2", "1000", 30.0, 120.0, 100),
		},
		ByCallsign: map[string]fsdfeed.Position{
			"CCA1": airborne("CCA1", "1002", 30.1, 120.0, 100),
		},
	}})

	if _, err := r.Fanout(speaker.sess.ID, packet(121800, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(obs.got) != 1 {
		t.Fatal("an observer's position comes from the aircraft they follow, not from their own cid")
	}
}

// TestAnObserverIsFilteredByTheAircraftTheyFollow 是上一条的负面配对。
//
// 上一条把观察员跟随的飞机放在说话人旁边，所以哪怕 lookup 完全忽略 Follow、
// 退回去查 ByCID 找不到、于是"位置未知→放行"，那个测试也一样是绿的。
// 这一条把跟随的飞机放到射程外：只有真的按 Follow 查到了位置，才会被过滤掉。
func TestAnObserverIsFilteredByTheAircraftTheyFollow(t *testing.T) {
	r := New()
	speaker := newRecorder(t, r, "1000")
	obs := &recorder{}
	obs.sess = r.Add(SessionOpts{
		CID: "9999", Follow: "CCA1", MaxTX: 0, MaxRX: 64,
		Send: func(b []byte) { obs.got = append(obs.got, append([]byte(nil), b...)) },
	})
	r.Subscribe(speaker.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(obs.sess.ID, control.Sub{RX: []uint32{121800}})

	// 跟随的飞机在 480 海里外，双方半项合计 40 海里。
	r.SetLocator(stubLocator{snap: fsdfeed.Snapshot{
		ByCID: map[string]fsdfeed.Position{
			"1000": airborne("CCA2", "1000", 30.0, 120.0, 20),
		},
		ByCallsign: map[string]fsdfeed.Position{
			"CCA1": airborne("CCA1", "1002", 38.0, 120.0, 20),
		},
	}})

	if _, err := r.Fanout(speaker.sess.ID, packet(121800, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(obs.got) != 0 {
		t.Fatal("an observer must be filtered by the position of the aircraft they follow; ignoring Follow makes every observer look position-unknown and therefore always in range")
	}
}

// --- 交叉耦合 ---

// TestCrossCoupleCarriesAnyonesTransmission 是宽读的核心。
//
// spec 第 8 节第 5 步写"应用该发送方的 xc 规则"，而同一页的散文写"客户端把 A
// 频率**收到的**音频重新发到 B"。按窄读实现的话，A 上的飞行员说的话到不了 B，
// 两个频率并没有合成一个会话——而交叉耦合在管制上的本义就是合成（清闲时段
// 一个人同时管 DEL 和 GND），can-audio 今天实现的也是合成。
func TestCrossCoupleCarriesAnyonesTransmission(t *testing.T) {
	r := New()
	ctl := newRecorder(t, r, "1000")    // 耦合 121800 和 124550 的管制员
	pilotA := newRecorder(t, r, "1001") // 在 121800 上说话的飞行员
	pilotB := newRecorder(t, r, "1002") // 只订阅 124550 的飞行员

	r.Subscribe(ctl.sess.ID, control.Sub{
		RX: []uint32{121800, 124550},
		TX: []uint32{121800, 124550},
		XC: [][2]uint32{{121800, 124550}},
	})
	r.Subscribe(pilotA.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(pilotB.sess.ID, control.Sub{RX: []uint32{124550}})

	if _, err := r.Fanout(pilotA.sess.ID, packet(121800, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(pilotB.got) != 1 {
		t.Fatal("a pilot on 124550 must hear a transmission on the coupled 121800; otherwise cross-coupling does not merge the two frequencies and it is a regression against can-audio")
	}
	h, _, _ := wire.Parse(pilotB.got[0])
	if h.FreqKHz != 124550 {
		t.Fatalf("FreqKHz = %d, want 124550 — the header must name the frequency this listener actually subscribed to, so their client routes it to the right radio", h.FreqKHz)
	}
}

// TestCrossCoupleDeliversOneCopyToSomeoneOnBothFrequencies 钉住去重。
// 不去重的话，同时订阅两个耦合频率的管制员会收到两份——回声/双声。
//
// TX 是必须的：耦合只有在两个频率都在该会话的 TX 授权集合里时才生效
// （见 TestAnUngrantedCrossCoupleIsRefused）。只声明 RX 的话这一对会被拒，
// 于是根本没有第二个目标频率，这个测试就变成了空测。
func TestCrossCoupleDeliversOneCopyToSomeoneOnBothFrequencies(t *testing.T) {
	r := New()
	ctl := newRecorder(t, r, "1000")
	pilot := newRecorder(t, r, "1001")
	r.Subscribe(ctl.sess.ID, control.Sub{
		RX: []uint32{121800, 124550},
		TX: []uint32{121800, 124550},
		XC: [][2]uint32{{121800, 124550}},
	})
	r.Subscribe(pilot.sess.ID, control.Sub{TX: []uint32{121800}})

	// 前提：耦合真的生效了，否则下面的"只收到一份"是因为压根没有第二个频率。
	if got := r.coupledWith(121800); len(got) != 1 || got[0] != 124550 {
		t.Fatalf("coupledWith(121800) = %v, want [124550] — without the coupling this test proves nothing", got)
	}

	n, err := r.Fanout(pilot.sess.ID, packet(121800, 1, 0xAA))
	if err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(ctl.got) != 1 {
		t.Fatalf("the controller got %d copies, want exactly 1 — two copies of one transmission is an echo", len(ctl.got))
	}
	if n != 1 {
		t.Fatalf("delivered = %d, want 1", n)
	}
	h, _, _ := wire.Parse(ctl.got[0])
	if h.FreqKHz != 121800 {
		t.Fatalf("FreqKHz = %d, want the primary frequency 121800 to win over the coupled one", h.FreqKHz)
	}
}

// TestCoupledTargetsAreVisitedInAscendingOrder 钉住目标频率的顺序。
//
// 目标是"主频率在前，耦合频率升序在后"，而这个顺序是客户端可见的：同时订阅了
// 两个**耦合**频率（都不是主频率）的人，包头上写的是先被命中的那一个，他的客户端
// 据此把音频送到哪一部电台。coupledWith 遍历的是 map，Go 的 map 遍历是随机的，
// 不排序的话同一个人在两次发言里会看到音频在两部电台之间跳。
func TestCoupledTargetsAreVisitedInAscendingOrder(t *testing.T) {
	r := New()
	ctl := newRecorder(t, r, "1000")
	pilot := newRecorder(t, r, "1001")
	both := newRecorder(t, r, "1002")

	// 118000 同时耦合到 121800 和 124550。
	r.Subscribe(ctl.sess.ID, control.Sub{
		RX: []uint32{118000, 121800, 124550},
		TX: []uint32{118000, 121800, 124550},
		XC: [][2]uint32{{118000, 121800}, {118000, 124550}},
	})
	r.Subscribe(pilot.sess.ID, control.Sub{TX: []uint32{118000}})
	// 这个人不订阅主频率，只订阅两个耦合频率。
	r.Subscribe(both.sess.ID, control.Sub{RX: []uint32{121800, 124550}})

	if _, err := r.Fanout(pilot.sess.ID, packet(118000, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(both.got) != 1 {
		t.Fatalf("got %d copies, want 1", len(both.got))
	}
	h, _, _ := wire.Parse(both.got[0])
	if h.FreqKHz != 121800 {
		t.Fatalf("FreqKHz = %d, want the lower coupled frequency 121800 — the target order must be deterministic, or the same listener sees the audio jump between two radios from one transmission to the next", h.FreqKHz)
	}
}

// TestCrossCoupleDoesNotChain 钉住"只走一跳"。
func TestCrossCoupleDoesNotChain(t *testing.T) {
	r := New()
	ctl := newRecorder(t, r, "1000")
	pilot := newRecorder(t, r, "1001")
	onC := newRecorder(t, r, "1002")

	// A↔B 和 B↔C，但 A 不该到 C。
	r.Subscribe(ctl.sess.ID, control.Sub{
		RX: []uint32{121800, 124550, 127800},
		TX: []uint32{121800, 124550, 127800},
		XC: [][2]uint32{{121800, 124550}, {124550, 127800}},
	})
	r.Subscribe(pilot.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(onC.sess.ID, control.Sub{RX: []uint32{127800}})

	// 前提：两对耦合都生效了。都被拒的话下面的断言自动成立，什么都没测到。
	if got := r.coupledWith(124550); len(got) != 2 {
		t.Fatalf("coupledWith(124550) = %v, want both pairs in effect", got)
	}

	if _, err := r.Fanout(pilot.sess.ID, packet(121800, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(onC.got) != 0 {
		t.Fatal("cross-coupling is one hop only; A↔B and B↔C must not make A reach C, or a declared cycle would loop on a path that runs tens of thousands of times a second")
	}
}

// TestUnsubscribingRemovesTheCoupling 钉住耦合索引跟着全量声明走。
func TestUnsubscribingRemovesTheCoupling(t *testing.T) {
	r := New()
	ctl := newRecorder(t, r, "1000")
	pilot := newRecorder(t, r, "1001")
	onB := newRecorder(t, r, "1002")

	r.Subscribe(ctl.sess.ID, control.Sub{
		RX: []uint32{121800, 124550},
		TX: []uint32{121800, 124550},
		XC: [][2]uint32{{121800, 124550}},
	})
	r.Subscribe(pilot.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(onB.sess.ID, control.Sub{RX: []uint32{124550}})

	if _, err := r.Fanout(pilot.sess.ID, packet(121800, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(onB.got) != 1 {
		t.Fatal("the coupling should be in effect")
	}

	// 管制员重发一份不含 XC 的全量声明。
	r.Subscribe(ctl.sess.ID, control.Sub{RX: []uint32{121800, 124550}, TX: []uint32{121800, 124550}})
	if _, err := r.Fanout(pilot.sess.ID, packet(121800, 2, 0xBB)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(onB.got) != 1 {
		t.Fatal("the coupling must be gone after a declaration that does not include it; SUB is a whole declaration, not a delta")
	}
}

// TestRemovingASessionRemovesItsCoupling 钉住会话消失时耦合也跟着消失。
//
// 和上一条不是一回事：上一条走 Subscribe 的重建路径，这一条走 removeLocked。
// 漏掉后者的话，一个管制员断线之后他的耦合永远留在索引里——两个频率从此
// 被永久接通，而声明它的那个人已经不在了。
func TestRemovingASessionRemovesItsCoupling(t *testing.T) {
	r := New()
	ctl := newRecorder(t, r, "1000")
	pilot := newRecorder(t, r, "1001")
	onB := newRecorder(t, r, "1002")

	r.Subscribe(ctl.sess.ID, control.Sub{
		RX: []uint32{121800, 124550},
		TX: []uint32{121800, 124550},
		XC: [][2]uint32{{121800, 124550}},
	})
	r.Subscribe(pilot.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(onB.sess.ID, control.Sub{RX: []uint32{124550}})

	r.Remove(ctl.sess.ID)

	if got := r.coupledWith(121800); len(got) != 0 {
		t.Fatalf("coupledWith(121800) = %v, want empty after the only declaring session was removed", got)
	}
	if _, err := r.Fanout(pilot.sess.ID, packet(121800, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(onB.got) != 0 {
		t.Fatal("a disconnected controller's cross-couple must not keep bridging two frequencies forever")
	}
}

// TestTwoControllersDeclaringTheSameCouplingKeepItWhenOneLeaves 钉住引用计数。
func TestTwoControllersDeclaringTheSameCouplingKeepItWhenOneLeaves(t *testing.T) {
	r := New()
	first := newRecorder(t, r, "1000")
	second := newRecorder(t, r, "1001")
	pilot := newRecorder(t, r, "1002")
	onB := newRecorder(t, r, "1003")

	both := control.Sub{
		RX: []uint32{121800, 124550},
		TX: []uint32{121800, 124550},
		XC: [][2]uint32{{121800, 124550}},
	}
	r.Subscribe(first.sess.ID, both)
	r.Subscribe(second.sess.ID, both)
	r.Subscribe(pilot.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(onB.sess.ID, control.Sub{RX: []uint32{124550}})

	r.Remove(first.sess.ID)

	if _, err := r.Fanout(pilot.sess.ID, packet(121800, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(onB.got) != 1 {
		t.Fatal("one controller leaving must not tear down a coupling another controller still declares; the index needs a reference count, not a boolean")
	}
}

// TestADuplicateCouplingDeclarationIsCountedOnce 钉住"一份重复的声明照样被一份
// 替代声明整体抵消"——加和减必须对称。
//
// 说清楚它**没有**钉住什么：把 normaliseXC 里的去重拿掉、或者把每对内部的升序
// 拿掉，这个测试都仍然是绿的（两个都实测过）。加走 next.xc、减走 old.xc，是同一份
// 列表，三加三减照样归零；bumpXC 对每一对同时动两个方向，所以对的朝向到不了索引；
// coupledWith 遍历 map，重复只会让计数变大，不会让它返回重复的频率。
// 真正会让它红的是破坏对称性的改动：把计数换成布尔、或者 Subscribe / removeLocked
// 漏掉那次减一。
func TestADuplicateCouplingDeclarationIsCountedOnce(t *testing.T) {
	r := New()
	ctl := newRecorder(t, r, "1000")
	pilot := newRecorder(t, r, "1001")
	onB := newRecorder(t, r, "1002")

	// 同一份声明里写了三遍，其中一遍反着写——都是同一对。
	r.Subscribe(ctl.sess.ID, control.Sub{
		RX: []uint32{121800, 124550},
		TX: []uint32{121800, 124550},
		XC: [][2]uint32{{121800, 124550}, {121800, 124550}, {124550, 121800}},
	})
	r.Subscribe(pilot.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(onB.sess.ID, control.Sub{RX: []uint32{124550}})

	// 三份声明只该指向一个目标频率。
	if got := r.coupledWith(121800); len(got) != 1 {
		t.Fatalf("coupledWith(121800) = %v, want exactly one target", got)
	}

	if _, err := r.Fanout(pilot.sess.ID, packet(121800, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(onB.got) != 1 {
		t.Fatalf("the listener got %d copies, want 1", len(onB.got))
	}

	// 撤销一次就该彻底撤销。计数写错的话这里会留下 2 或 3 的残值。
	r.Subscribe(ctl.sess.ID, control.Sub{RX: []uint32{121800, 124550}, TX: []uint32{121800, 124550}})
	if _, err := r.Fanout(pilot.sess.ID, packet(121800, 2, 0xBB)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(onB.got) != 1 {
		t.Fatal("a declaration repeated three times must still be undone by one replacement; the reference count leaked")
	}
}

// TestAnUngrantedCrossCoupleIsRefused 是宽读带来的权限检查。
//
// 耦合是全服务端生效的：任何人在 A 上发言都会到达 B。所以不校验的话，一条普通
// 的飞行员会话可以声明 XC: [[121800, 123450]]，把两个不相干的频率接通，而它这
// 两个频率一个都没有。校验的规则和无线电栈本来的耦合规则一致——打开 XC 会强制
// RX 和 TX 都打开，也就是说你只能把自己正在发射的频率接起来。
func TestAnUngrantedCrossCoupleIsRefused(t *testing.T) {
	r := New()
	rogue := newRecorder(t, r, "1000")
	speaker := newRecorder(t, r, "1001")
	victim := newRecorder(t, r, "1002")

	// rogue 只在 118000 上发射，却想把 121800 和 123450 接起来。
	ack := r.Subscribe(rogue.sess.ID, control.Sub{
		TX: []uint32{118000},
		XC: [][2]uint32{{121800, 123450}},
	})
	if len(ack.RejectedXC) != 1 {
		t.Fatalf("RejectedXC = %v, want the one ungranted pair — a session must not be able to couple frequencies it does not transmit on", ack.RejectedXC)
	}

	r.Subscribe(speaker.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(victim.sess.ID, control.Sub{RX: []uint32{123450}})

	if _, err := r.Fanout(speaker.sess.ID, packet(121800, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(victim.got) != 0 {
		t.Fatal("a third party's ungranted cross-couple declaration carried audio onto an unrelated frequency")
	}
}

// TestACrossCoupleRejectedByMaxTXDoesNotTakeEffect 钉住权限检查用的是**授权后**
// 的 TX 集合，不是客户端声明的那一份——否则 MaxTX 限额可以被 XC 绕过。
func TestACrossCoupleRejectedByMaxTXDoesNotTakeEffect(t *testing.T) {
	r := New()
	ctl := &recorder{}
	ctl.sess = r.Add(SessionOpts{
		CID: "1000", MaxTX: 1, MaxRX: 64,
		Send: func(b []byte) { ctl.got = append(ctl.got, append([]byte(nil), b...)) },
	})
	speaker := newRecorder(t, r, "1001")
	victim := newRecorder(t, r, "1002")

	// 声明两个 TX，但 MaxTX 是 1——124550 会被拒，所以那一对耦合也不该生效。
	ack := r.Subscribe(ctl.sess.ID, control.Sub{
		TX: []uint32{121800, 124550},
		XC: [][2]uint32{{121800, 124550}},
	})
	if len(ack.Rejected) != 1 || ack.Rejected[0] != 124550 {
		t.Fatalf("Rejected = %v, want [124550]", ack.Rejected)
	}
	if len(ack.RejectedXC) != 1 {
		t.Fatalf("RejectedXC = %v, want the pair whose second frequency was refused — validating against the declared TX instead of the granted one lets MaxTX be bypassed", ack.RejectedXC)
	}

	r.Subscribe(speaker.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(victim.sess.ID, control.Sub{RX: []uint32{124550}})
	if _, err := r.Fanout(speaker.sess.ID, packet(121800, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(victim.got) != 0 {
		t.Fatal("the coupling took effect despite its second frequency being refused by MaxTX")
	}
}

// TestASelfCoupledPairIsReportedNotSilentlyDropped 确认自耦合也回报。
func TestASelfCoupledPairIsReportedNotSilentlyDropped(t *testing.T) {
	r := New()
	s := newRecorder(t, r, "1000")
	ack := r.Subscribe(s.sess.ID, control.Sub{TX: []uint32{121800}, XC: [][2]uint32{{121800, 121800}}})
	if len(ack.RejectedXC) != 1 {
		t.Fatalf("RejectedXC = %v, want the self-loop reported", ack.RejectedXC)
	}
	if got := r.coupledWith(121800); len(got) != 0 {
		t.Fatalf("coupledWith(121800) = %v, want empty — a frequency coupled to itself would put the same listener in the target list twice", got)
	}
}

// TestAnOverlongCrossCoupleListIsTruncated 钉住长度上界。
// 去重要先建 map，一份带一百万个对的声明会先分配一百万个条目。
func TestAnOverlongCrossCoupleListIsTruncated(t *testing.T) {
	r := New()
	s := newRecorder(t, r, "1000")
	pairs := make([][2]uint32, 0, maxXCPairs+10)
	for i := 0; i < maxXCPairs+10; i++ {
		pairs = append(pairs, [2]uint32{118000, uint32(121800 + i)})
	}
	ack := r.Subscribe(s.sess.ID, control.Sub{TX: []uint32{118000}, XC: pairs})
	if len(ack.RejectedXC) != len(pairs) {
		// 这里全部都会被拒（第二个频率都不在 TX 里），关键是不能 panic、
		// 不能吞掉，而且超过上界的那些也要出现在回报里。
		t.Fatalf("RejectedXC = %d entries, want all %d", len(ack.RejectedXC), len(pairs))
	}
}

// TestAnOverlongListOfGrantableCrossCouplesStopsAtTheCap 是上一条真正测到上界的
// 那一半：上一条的对全都因为 TX 不含第二个频率而被拒，就算根本没有上界检查，
// 被拒的数量也一样对得上。这一条让每一对都**可以**被授权，于是只有上界能拦住它。
func TestAnOverlongListOfGrantableCrossCouplesStopsAtTheCap(t *testing.T) {
	r := New()
	tx := make([]uint32, 0, maxXCPairs+11)
	tx = append(tx, 118000)
	pairs := make([][2]uint32, 0, maxXCPairs+10)
	for i := 0; i < maxXCPairs+10; i++ {
		f := uint32(121800 + i)
		tx = append(tx, f)
		pairs = append(pairs, [2]uint32{118000, f})
	}
	s := r.Add(SessionOpts{CID: "1000", MaxTX: len(tx), MaxRX: len(tx), Send: func([]byte) {}})
	ack := r.Subscribe(s.ID, control.Sub{TX: tx, XC: pairs})

	if got := len(r.coupledWith(118000)); got != maxXCPairs {
		t.Fatalf("coupledWith(118000) has %d entries, want the cap %d", got, maxXCPairs)
	}
	if len(ack.RejectedXC) != 10 {
		t.Fatalf("RejectedXC = %d entries, want the 10 past the cap reported rather than silently dropped", len(ack.RejectedXC))
	}
}

// TestAHostileCrossCoupleListStillProducesASendableAck 钉住回报本身有界。
//
// "被拒的对必须告诉客户端"和"ACK 必须发得出去"是同一条纪律的两半。SubAck 走
// control.WriteFrame，上限和入站一样是 64 KiB；一份塞满上限的 SUB 能带七千多个
// 耦合对，把它们原样抄进 RejectedXC 会让 ACK 超限而根本发不出去——客户端于是
// 什么都收不到，比静默丢弃还糟。
func TestAHostileCrossCoupleListStillProducesASendableAck(t *testing.T) {
	r := New()
	s := newRecorder(t, r, "1000")
	pairs := make([][2]uint32, 0, 8000)
	for i := 0; i < 8000; i++ {
		pairs = append(pairs, [2]uint32{118000, uint32(118001 + i)})
	}
	ack := r.Subscribe(s.sess.ID, control.Sub{TX: []uint32{118000}, XC: pairs})

	if len(ack.RejectedXC) > 2*maxXCPairs {
		t.Fatalf("RejectedXC = %d entries, want at most %d", len(ack.RejectedXC), 2*maxXCPairs)
	}
	if len(ack.RejectedXC) == 0 {
		t.Fatal("the refusal must still be reported; silently dropping it is the failure this whole rule exists to avoid")
	}
	b, err := control.Encode(&ack)
	if err != nil {
		t.Fatalf("Encode: %v", err)
	}
	if len(b) > control.MaxFrame {
		t.Fatalf("the SUBACK is %d bytes, over the %d byte frame limit — it could never be sent", len(b), control.MaxFrame)
	}
}

func TestFanoutRejectsAMalformedPacket(t *testing.T) {
	r := New()
	speaker := newRecorder(t, r, "1000")
	r.Subscribe(speaker.sess.ID, control.Sub{TX: []uint32{121800}})
	if _, err := r.Fanout(speaker.sess.ID, []byte{0x01, 0x02}); err == nil {
		t.Fatal("a packet shorter than the header must be rejected")
	}
}

func TestFanoutRejectsAGoneSession(t *testing.T) {
	r := New()
	speaker := newRecorder(t, r, "1000")
	r.Subscribe(speaker.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Remove(speaker.sess.ID)
	if _, err := r.Fanout(speaker.sess.ID, packet(121800, 1)); err == nil {
		t.Fatal("a removed session must not be able to transmit")
	}
}

// TestFanoutGivesEachListenerItsOwnBuffer 钉住"每个听众一份新缓冲"。
//
// Send 可能是异步的——传输层把切片塞进 QUIC 的发送队列然后就返回了。共用一份
// 缓冲的话，第二个听众的包头会覆盖第一个听众还没发出去的那一份，而两个人的
// qual 和 freq 本来就不一样。表现是"偶尔有人的音频出现在别的频率上"。
//
// 这个测试**不能**用 newRecorder：它的 Send 立刻把字节复制一份，于是两个听众
// 的切片必然不同源，共不共缓冲都测不出来。这里存的是原切片。
func TestFanoutGivesEachListenerItsOwnBuffer(t *testing.T) {
	r := New()
	speaker := newRecorder(t, r, "1000")

	var raw [][]byte
	keep := func(b []byte) { raw = append(raw, b) }
	one := r.Add(SessionOpts{CID: "1001", MaxTX: 8, MaxRX: 64, Send: keep})
	two := r.Add(SessionOpts{CID: "1002", MaxTX: 8, MaxRX: 64, Send: keep})

	r.Subscribe(speaker.sess.ID, control.Sub{TX: []uint32{121800}})
	r.Subscribe(one.ID, control.Sub{RX: []uint32{121800}})
	r.Subscribe(two.ID, control.Sub{RX: []uint32{121800}})

	if _, err := r.Fanout(speaker.sess.ID, packet(121800, 1, 0xAA)); err != nil {
		t.Fatalf("Fanout: %v", err)
	}
	if len(raw) != 2 {
		t.Fatalf("got %d packets, want 2", len(raw))
	}
	if &raw[0][0] == &raw[1][0] {
		t.Fatal("both listeners were handed the same backing array; Send may be asynchronous, so the second header would overwrite the first packet before it is on the wire")
	}
}
