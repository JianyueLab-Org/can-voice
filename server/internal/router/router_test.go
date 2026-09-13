package router

import (
	"slices"
	"sync"
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/control"
)

func newSession(t *testing.T, r *Router, cid string, maxTX int) *Session {
	t.Helper()
	// maxRX 给一个宽松的默认值，只有专门测它的用例才自己调 Add。
	s := r.Add(cid, "", maxTX, 64, func([]byte) {})
	if s == nil {
		t.Fatal("Add returned nil")
	}
	return s
}

func TestSubscribeReplacesWholesaleRatherThanMerging(t *testing.T) {
	// SUB 是全量声明。第二次订阅必须整体取代第一次，
	// 而不是并集——任何增量语义都会把 sync 风暴那一类 bug 请回来。
	r := New()
	s := newSession(t, r, "1000", 8)

	r.Subscribe(s.ID, control.Sub{RX: []uint32{118000, 121800}})
	r.Subscribe(s.ID, control.Sub{RX: []uint32{124550}})

	if got := r.Listeners(118000); len(got) != 0 {
		t.Fatalf("118000 still has %d listeners; the second SUB must replace the first, not merge", len(got))
	}
	if got := r.Listeners(124550); len(got) != 1 {
		t.Fatalf("124550 has %d listeners, want 1", len(got))
	}
}

func TestTxImpliesRx(t *testing.T) {
	// 没有"只发不收"的电台（无线电栈耦合规则，spec 9.1）。
	r := New()
	s := newSession(t, r, "1000", 8)
	ack := r.Subscribe(s.ID, control.Sub{RX: []uint32{118000}, TX: []uint32{121800}})

	if len(r.Listeners(121800)) != 1 {
		t.Fatal("a frequency declared for TX must also be received")
	}
	if !contains(ack.RX, 121800) {
		t.Fatalf("SubAck.RX = %v, must include the TX frequency", ack.RX)
	}
}

func TestSubscribeRejectsTxBeyondMaxTX(t *testing.T) {
	r := New()
	s := newSession(t, r, "1000", 2)
	ack := r.Subscribe(s.ID, control.Sub{TX: []uint32{118000, 121800, 124550}})

	if len(ack.TX) != 2 {
		t.Fatalf("accepted TX = %v, want 2 (max_tx from the token)", ack.TX)
	}
	if len(ack.Rejected) != 1 {
		t.Fatalf("Rejected = %v, want exactly the one over the limit", ack.Rejected)
	}
	if r.MayTransmit(s.ID, ack.Rejected[0]) {
		t.Fatal("a rejected frequency must not be transmittable")
	}
}

func TestMayTransmitOnlyOnDeclaredFrequencies(t *testing.T) {
	r := New()
	s := newSession(t, r, "1000", 8)
	r.Subscribe(s.ID, control.Sub{RX: []uint32{118000, 121800}, TX: []uint32{121800}})

	if !r.MayTransmit(s.ID, 121800) {
		t.Fatal("121800 was declared for TX")
	}
	if r.MayTransmit(s.ID, 118000) {
		t.Fatal("118000 is RX only; transmitting on it must be refused")
	}
	if r.MayTransmit(s.ID, 999000) {
		t.Fatal("an undeclared frequency must not be transmittable")
	}
}

func TestRemoveClearsEveryFrequency(t *testing.T) {
	r := New()
	s := newSession(t, r, "1000", 8)
	r.Subscribe(s.ID, control.Sub{RX: []uint32{118000, 121800, 124550}})
	r.Remove(s.ID)

	for _, f := range []uint32{118000, 121800, 124550} {
		if got := r.Listeners(f); len(got) != 0 {
			t.Fatalf("%d still has %d listeners after Remove", f, len(got))
		}
	}
}

func TestSubscribeDeduplicatesFrequencies(t *testing.T) {
	r := New()
	s := newSession(t, r, "1000", 8)
	r.Subscribe(s.ID, control.Sub{RX: []uint32{118000, 118000, 118000}})
	if got := r.Listeners(118000); len(got) != 1 {
		t.Fatalf("a repeated frequency produced %d listener entries, want 1", len(got))
	}
}

// TestSubscribeIsRaceFreeAgainstConcurrentReaders 是这个包最重要的测试。
//
// Listeners() 把 *Session 指针交给调用方，而扇出路径（Task 8）会在锁外读这些
// 会话的订阅状态——(*Session).crossCoupled 就是不持锁地遍历 s.xc。如果订阅状态
// 是被互斥锁保护的普通字段，那就是切片头的并发读写：读到撕裂的 ptr/len 组合，
// 结果是音频被转发到一个随机频率，或者直接越界 panic 把服务端带下去。
// 订阅状态因此必须是"一次声明一个不可变值、整体原子替换"，这也正好就是
// 全量 SUB 的设计本身——数据结构把设计原则表达出来了。
//
// 必须用 `go test -race` 跑才有意义。
func TestSubscribeIsRaceFreeAgainstConcurrentReaders(t *testing.T) {
	r := New()
	s := newSession(t, r, "1000", 8)
	r.Subscribe(s.ID, control.Sub{RX: []uint32{118000}, TX: []uint32{118000}})

	var wg sync.WaitGroup
	stop := make(chan struct{})

	// 写者：不停地整体替换订阅声明。
	wg.Add(1)
	go func() {
		defer wg.Done()
		freqs := []uint32{118000, 121800, 124550, 127800}
		for i := 0; ; i++ {
			select {
			case <-stop:
				return
			default:
			}
			f := freqs[i%len(freqs)]
			g := freqs[(i+1)%len(freqs)]
			r.Subscribe(s.ID, control.Sub{
				RX: []uint32{f, g},
				TX: []uint32{f},
				XC: [][2]uint32{{f, g}},
			})
		}
	}()

	// 读者：走扇出会走的那条路——先 Listeners 拿到指针，再在锁外读订阅状态。
	for i := 0; i < 4; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for {
				select {
				case <-stop:
					return
				default:
				}
				for _, freq := range []uint32{118000, 121800, 124550, 127800} {
					for _, sess := range r.Listeners(freq) {
						// 这两个调用刻意在 Listeners 返回之后、不持任何锁。
						_ = sess.crossCoupled(freq)
						_ = r.MayTransmit(sess.ID, freq)
					}
				}
			}
		}()
	}

	time.Sleep(200 * time.Millisecond)
	close(stop)
	wg.Wait()
}

// TestSubAckOrderingIsDeterministic 钉住 ACK 的顺序。
//
// 原实现用 `for f := range newRX` 构造 ack.RX，而 Go 的 map 遍历是随机的——
// 同一份 SUB 在两次运行里会得到顺序不同的 ACK。客户端要拿 ACK 和自己的声明
// 对账（哪些被拒了），顺序不稳定会让对账逻辑和测试都间歇性地翻车。
func TestSubAckOrderingIsDeterministic(t *testing.T) {
	sub := control.Sub{
		RX: []uint32{127800, 118000, 124550, 121800},
		TX: []uint32{124550, 118000},
	}
	r := New()
	s := newSession(t, r, "1000", 8)
	want := r.Subscribe(s.ID, sub)

	if !slices.IsSorted(want.RX) {
		t.Fatalf("SubAck.RX = %v, want ascending order", want.RX)
	}
	for i := 0; i < 30; i++ {
		r2 := New()
		s2 := newSession(t, r2, "1000", 8)
		got := r2.Subscribe(s2.ID, sub)
		if !slices.Equal(got.RX, want.RX) {
			t.Fatalf("run %d: RX = %v, first run = %v — the ack depends on map iteration order", i, got.RX, want.RX)
		}
	}
}

// TestSubscribeCopiesTheCrossCoupleList 确认 router 不会把调用方的切片存下来。
// 存下来的话，控制面那边复用或修改缓冲区就会改到服务端的路由状态。
func TestSubscribeCopiesTheCrossCoupleList(t *testing.T) {
	r := New()
	s := newSession(t, r, "1000", 8)
	xc := [][2]uint32{{121800, 124550}}
	r.Subscribe(s.ID, control.Sub{TX: []uint32{121800}, XC: xc})

	xc[0] = [2]uint32{999000, 999001} // 调用方改自己的切片

	if got := s.crossCoupled(121800); len(got) != 1 || got[0] != 124550 {
		t.Fatalf("crossCoupled = %v, want [124550] — Subscribe aliased the caller's slice instead of copying it", got)
	}
}

// TestSubscribeRejectsRxBeyondMaxRX 钉住 RX 上限真的被强制。
//
// Task 9 的 Config 里有 MaxRX，而且在 READY 里告诉了客户端，但如果这里不拦，
// 那个数字就只是一句建议。每个 RX 频率都要写进 Router 的倒排索引，而那是在
// 写锁里做的——一个声明了一万个频率的会话会让全网的扇出排队等它。
func TestSubscribeRejectsRxBeyondMaxRX(t *testing.T) {
	r := New()
	s := r.Add("1000", "", 8, 2, func([]byte) {})
	ack := r.Subscribe(s.ID, control.Sub{RX: []uint32{118000, 121800, 124550, 127800}})

	if len(ack.RX) != 2 {
		t.Fatalf("accepted RX = %v, want 2 (max_rx from the server config)", ack.RX)
	}
	if len(ack.Rejected) != 2 {
		t.Fatalf("Rejected = %v, want the two over the limit", ack.Rejected)
	}
	var listening int
	for _, f := range []uint32{118000, 121800, 124550, 127800} {
		listening += len(r.Listeners(f))
	}
	if listening != 2 {
		t.Fatalf("the session is indexed on %d frequencies, want 2 — a rejected frequency must not reach the inverted index", listening)
	}
}

// TestTxIsNotSqueezedOutByTheRxLimit 钉住上一条的边界。
// TX ⊆ RX，所以 TX 频率会先占掉 RX 名额；一个 max_tx 大于 max_rx 的 token
// 不该让自己声明的 TX 被自己的 RX 上限挤掉。
func TestTxIsNotSqueezedOutByTheRxLimit(t *testing.T) {
	r := New()
	s := r.Add("1000", "", 2, 2, func([]byte) {})
	r.Subscribe(s.ID, control.Sub{RX: []uint32{127800, 128000}, TX: []uint32{118000, 121800}})

	for _, f := range []uint32{118000, 121800} {
		if !r.MayTransmit(s.ID, f) {
			t.Fatalf("%d was declared for TX and accepted, but is not transmittable", f)
		}
		if len(r.Listeners(f)) != 1 {
			t.Fatalf("%d is a TX frequency and must therefore also be received", f)
		}
	}
}

// TestASessionWithNoSubscriptionYetIsSafeToRead 钉住 Add 之后、第一次 Subscribe
// 之前那段窗口。扇出在那段时间里完全可能已经拿到这个会话。
func TestASessionWithNoSubscriptionYetIsSafeToRead(t *testing.T) {
	r := New()
	s := newSession(t, r, "1000", 8)
	if got := s.crossCoupled(118000); len(got) != 0 {
		t.Fatalf("crossCoupled = %v, want empty", got)
	}
	if r.MayTransmit(s.ID, 118000) {
		t.Fatal("a session that has not subscribed must not be able to transmit")
	}
	if len(r.Listeners(118000)) != 0 {
		t.Fatal("a session that has not subscribed must not be a listener")
	}
}

func contains(xs []uint32, v uint32) bool {
	for _, x := range xs {
		if x == v {
			return true
		}
	}
	return false
}
