package router

import (
	"fmt"
	"runtime"
	"slices"
	"sync"
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/control"
)

func newSession(t *testing.T, r *Router, cid string, maxTX int) *Session {
	t.Helper()
	// maxRX 给一个宽松的默认值，只有专门测它的用例才自己构造 SessionOpts。
	s := r.Add(SessionOpts{CID: cid, MaxTX: maxTX, MaxRX: 64, Send: func([]byte) {}})
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

// TestRetuningOneSessionLeavesTheOtherListenersAlone 钉住倒排索引的清理只清自己。
//
// unindex 里"最后一个订阅者走了就删掉整个频率条目"那一步，如果写成无条件删除，
// 全部单会话测试仍然会绿——而真实后果是一个人调走频率会把同频率上的其他人
// 一起从索引里抹掉，那个频率就此对所有人静默。
func TestRetuningOneSessionLeavesTheOtherListenersAlone(t *testing.T) {
	r := New()
	a := newSession(t, r, "1000", 8)
	b := newSession(t, r, "1001", 8)
	r.Subscribe(a.ID, control.Sub{RX: []uint32{118000}})
	r.Subscribe(b.ID, control.Sub{RX: []uint32{118000}})
	if got := len(r.Listeners(118000)); got != 2 {
		t.Fatalf("118000 has %d listeners, want 2", got)
	}

	// a 调到别的频率去。
	r.Subscribe(a.ID, control.Sub{RX: []uint32{121800}})

	if got := len(r.Listeners(118000)); got != 1 {
		t.Fatalf("118000 has %d listeners after one of two retuned, want 1 — retuning must not evict the other listener", got)
	}
	if r.Listeners(118000)[0].ID != b.ID {
		t.Fatal("the surviving listener is the wrong session")
	}
	if got := len(r.Listeners(121800)); got != 1 {
		t.Fatalf("121800 has %d listeners, want 1", got)
	}
}

// TestRemovingOneOfTwoListenersLeavesTheOther 是同一条，走 Remove 那条路径。
func TestRemovingOneOfTwoListenersLeavesTheOther(t *testing.T) {
	r := New()
	a := newSession(t, r, "1000", 8)
	b := newSession(t, r, "1001", 8)
	r.Subscribe(a.ID, control.Sub{RX: []uint32{118000}})
	r.Subscribe(b.ID, control.Sub{RX: []uint32{118000}})

	r.Remove(a.ID)

	if got := len(r.Listeners(118000)); got != 1 {
		t.Fatalf("118000 has %d listeners after one of two was removed, want 1", got)
	}
}

// TestAnEmptyDeclarationClearsEverything 钉住"全量声明"的下界。
//
// SUB 是整体替换，所以一份空声明的意思是"我什么都不收、什么都不发"。
// 在 Subscribe 开头加一句"空的就直接返回"会让全部现有测试保持绿，而一个
// 下班的管制员会继续收着他原来那一堆频率的音频。
func TestAnEmptyDeclarationClearsEverything(t *testing.T) {
	r := New()
	s := newSession(t, r, "1000", 8)
	r.Subscribe(s.ID, control.Sub{RX: []uint32{118000, 121800}, TX: []uint32{118000}})

	ack := r.Subscribe(s.ID, control.Sub{})

	for _, f := range []uint32{118000, 121800} {
		if got := len(r.Listeners(f)); got != 0 {
			t.Fatalf("%d still has %d listeners after an empty declaration", f, got)
		}
		if r.MayTransmit(s.ID, f) {
			t.Fatalf("%d is still transmittable after an empty declaration", f)
		}
	}
	if len(ack.RX) != 0 || len(ack.TX) != 0 {
		t.Fatalf("ack = %+v, want empty RX and TX", ack)
	}
}

// TestWhichFrequenciesSurviveALimitIsDeterministic 钉住超限时留下哪几个。
//
// 现有的排序断言只钉住了 ACK 的**顺序**，没钉住**内容**。把 dedup 换成基于 map
// 的写法（同样去重）会让全部现有测试保持绿，而同一份 SUB 在多次运行里会接受
// 不同的频率子集——客户端每次重连都落在不同的频率上。
//
// 规则是"按客户端声明的先后顺序接受，超出的拒绝"，所以答案唯一。
func TestWhichFrequenciesSurviveALimitIsDeterministic(t *testing.T) {
	sub := control.Sub{RX: []uint32{127800, 118000, 124550, 121800}}
	for i := 0; i < 50; i++ {
		r := New()
		s := r.Add(SessionOpts{CID: "1000", MaxTX: 8, MaxRX: 2, Send: func([]byte) {}})
		ack := r.Subscribe(s.ID, sub)
		// 声明顺序里的前两个：127800 和 118000。ACK 是排过序的。
		if len(ack.RX) != 2 || ack.RX[0] != 118000 || ack.RX[1] != 127800 {
			t.Fatalf("run %d: accepted RX = %v, want the first two in declaration order (127800, 118000)", i, ack.RX)
		}
		if len(r.Listeners(124550)) != 0 || len(r.Listeners(121800)) != 0 {
			t.Fatalf("run %d: a rejected frequency reached the inverted index", i)
		}
	}
}

// TestRejectedHasNoDuplicatesAcrossTxAndRxLimits 钉住 0e 的第一半：同一个频率
// 可以先被 TX 限额拒、又被 RX 限额拒（一个真实测出的形状：MaxTX=1/MaxRX=1 下
// Rejected = [121800 121800]），dedup 必须把 Rejected 自身的重复去掉。
func TestRejectedHasNoDuplicatesAcrossTxAndRxLimits(t *testing.T) {
	r := New()
	s := r.Add(SessionOpts{CID: "1000", MaxTX: 1, MaxRX: 1, Send: func([]byte) {}})
	// 118000 挤满 TX 名额；121800 先被 TX 限额拒，再作为纯 RX 声明被 RX 限额拒。
	ack := r.Subscribe(s.ID, control.Sub{TX: []uint32{118000, 121800}, RX: []uint32{121800}})

	var n int
	for _, f := range ack.Rejected {
		if f == 121800 {
			n++
		}
	}
	if n != 1 {
		t.Fatalf("Rejected = %v, want 121800 exactly once even though it was rejected by both the TX and RX limits", ack.Rejected)
	}
}

// TestATxRejectedFrequencyCanStillBeGrantedForRx 钉住 0e 的第二半：一个频率
// 同时出现在 RX 和 Rejected 里是有意义的（TX 被限额拒了，但 RX 给了），
// 不能被"修掉"——那样会丢掉信息（control.SubAck 的文档注释）。
func TestATxRejectedFrequencyCanStillBeGrantedForRx(t *testing.T) {
	r := New()
	s := r.Add(SessionOpts{CID: "1000", MaxTX: 1, MaxRX: 8, Send: func([]byte) {}})
	// 118000 占掉唯一的 TX 名额；121800 因此被 TX 限额拒，但显式声明的 RX 还有余量。
	ack := r.Subscribe(s.ID, control.Sub{TX: []uint32{118000, 121800}, RX: []uint32{121800}})

	if !contains(ack.RX, 121800) {
		t.Fatalf("SubAck.RX = %v, want 121800 present — it was granted for RX even though TX refused it", ack.RX)
	}
	if !contains(ack.Rejected, 121800) {
		t.Fatalf("Rejected = %v, want 121800 present — TX refused it", ack.Rejected)
	}
	if r.MayTransmit(s.ID, 121800) {
		t.Fatal("121800 was rejected for TX and must not be transmittable")
	}
	if len(r.Listeners(121800)) != 1 {
		t.Fatal("121800 was granted for RX and must be in the inverted index")
	}
}

// TestZeroMaxRXStillAllowsTxImpliedRx 钉住 0f 的第二条：MaxRX 为 0 时的行为
// 是有意的，不是碰巧。MaxRX=0 本不该发生——配置校验会拒绝非正的
// CAN_VOICE_MAX_RX（Task 11 的 LoadConfig）——但如果它真的是 0，纯 RX 声明
// 必须全部被拒，而 TX 蕴含的那些照常通过：TX 频率不受 RX 限额挤压
// （TestTxIsNotSqueezedOutByTheRxLimit 钉的是同一条规则）。
func TestZeroMaxRXStillAllowsTxImpliedRx(t *testing.T) {
	r := New()
	s := r.Add(SessionOpts{CID: "1000", MaxTX: 1, MaxRX: 0, Send: func([]byte) {}})
	ack := r.Subscribe(s.ID, control.Sub{RX: []uint32{121800}, TX: []uint32{118000}})

	if len(ack.RX) != 1 || ack.RX[0] != 118000 {
		t.Fatalf("SubAck.RX = %v, want just the TX-implied frequency 118000", ack.RX)
	}
	if !contains(ack.Rejected, 121800) {
		t.Fatalf("Rejected = %v, want the pure-RX frequency 121800 rejected under MaxRX=0", ack.Rejected)
	}
	if !r.MayTransmit(s.ID, 118000) {
		t.Fatal("the TX-implied frequency must still be transmittable when MaxRX is 0")
	}
	if len(r.Listeners(121800)) != 0 {
		t.Fatal("the pure-RX frequency must not reach the inverted index when MaxRX is 0")
	}
}

// TestSubscribeDeduplicatesFrequencies 钉住 0d，第二版。
//
// 上一版断言 ack.RX，前提是错的：ack.RX 是遍历 next.rx 这张 map 构造的，
// 和它取代的 r.Listeners() 计数一样会把重复吞掉，把 dedup 换成恒等函数
// 那个版本照样绿。
//
// 断言必须落在 ack.TX 上——它是按去重后的切片顺序 append 的，对重复敏感。
//
// 去重失效的真实后果是 ack.TX 里出现字面上的重复项：客户端拿 ACK 对账时
// 会看到同一个频率两次。它**不会**挤掉别的频率——闸门是 len(next.tx)，
// 而那是一张 map，重复写的是同一个 key，计数不增。（这一点我一开始写反了，
// 留个记号免得下一个人照着错的因果去找。）
func TestSubscribeDeduplicatesFrequencies(t *testing.T) {
	r := New()
	s := r.Add(SessionOpts{CID: "1000", MaxTX: 2, MaxRX: 64, Send: func([]byte) {}})

	ack := r.Subscribe(s.ID, control.Sub{TX: []uint32{118000, 118000, 121800}})

	if len(ack.TX) != 2 || ack.TX[0] != 118000 || ack.TX[1] != 121800 {
		t.Fatalf("SubAck.TX = %v, want [118000 121800] — a duplicate must not consume a MaxTX slot", ack.TX)
	}
	if len(ack.Rejected) != 0 {
		t.Fatalf("Rejected = %v, want empty — nothing here exceeds MaxTX=2 once duplicates are removed", ack.Rejected)
	}
	if !r.MayTransmit(s.ID, 121800) {
		t.Fatal("121800 was declared and fits within MaxTX; a duplicate of another frequency must not have squeezed it out")
	}
}

// TestADuplicateNeverLandsInBothAcceptedAndRejected 钉住一个自相矛盾的 ACK。
//
// 去重失效时，一个**已经被接受**的频率的重复项会在限额用满之后再次进入循环，
// 于是被追加进 Rejected——客户端同时收到"121800 已接受"和"121800 被拒"，
// 而 MayTransmit 说可以发。这和 RX 与 Rejected 的重叠不是一回事：那一种带着
// 信息（TX 被拒、RX 给了），这一种是 TX 这一个决定内部自相矛盾。
func TestADuplicateNeverLandsInBothAcceptedAndRejected(t *testing.T) {
	r := New()
	s := r.Add(SessionOpts{CID: "1000", MaxTX: 2, MaxRX: 64, Send: func([]byte) {}})
	ack := r.Subscribe(s.ID, control.Sub{TX: []uint32{118000, 121800, 121800}})

	for _, f := range ack.TX {
		for _, g := range ack.Rejected {
			if f == g {
				t.Fatalf("%d is in both TX %v and Rejected %v; the client is told it may and may not transmit on the same frequency", f, ack.TX, ack.Rejected)
			}
		}
	}
}

// TestSubscribeIsRaceFreeAgainstConcurrentReaders 是这个包最重要的测试。
//
// Listeners() 把 *Session 指针交给调用方，而扇出路径会在锁外读这些会话的订阅
// 状态——MayTransmit 取完会话指针就放锁，然后才 Load()。如果订阅状态是被互斥锁
// 保护的普通字段，那就是 map/切片头的并发读写：读到撕裂的 ptr/len 组合，
// 结果是音频被转发到一个随机频率，或者直接越界 panic 把服务端带下去。
// 订阅状态因此必须是"一次声明一个不可变值、整体原子替换"，这也正好就是
// 全量 SUB 的设计本身——数据结构把设计原则表达出来了。
//
// 读者跑的是真正的扇出（Fanout），不是它的某个零件：耦合索引、rx 倒排索引和
// 会话表在扇出的一次调用里被分四次加锁读到，写者同时在整体重建这三张表。
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
					// 扇出：写者随时在换声明，所以这里常常拿到"不能在这个
					// 频率上发送"的错误，那是预期的，丢掉即可。
					_, _ = r.Fanout(s.ID, packet(freq, 1, 0xAA))
					for _, sess := range r.Listeners(freq) {
						// 这个调用刻意在 Listeners 返回之后、不持任何锁。
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

// TestSubAckTxIsSortedToo 钉住 ack.TX 也按升序返回。
//
// 现有的顺序测试只查 ack.RX（它遍历 map，顺序天然随机，所以一眼就看得出要排）。
// ack.TX 是按声明顺序 append 的，看起来"本来就稳定"，于是排序那一行没人守——
// 但客户端拿 ACK 和自己的声明对账靠的是同一个契约，两个字段不该有两套规则。
func TestSubAckTxIsSortedToo(t *testing.T) {
	r := New()
	s := r.Add(SessionOpts{CID: "1000", MaxTX: 8, MaxRX: 64, Send: func([]byte) {}})
	ack := r.Subscribe(s.ID, control.Sub{TX: []uint32{124550, 118000, 121800}})

	want := []uint32{118000, 121800, 124550}
	if !slices.Equal(ack.TX, want) {
		t.Fatalf("SubAck.TX = %v, want %v — ACK fields are sorted so a client can reconcile against its own declaration", ack.TX, want)
	}
}

// TestSubAckRejectedIsSortedToo 补齐 ACK 三个列表字段里最后一个没人守的排序。
//
// 它是顺着声明顺序 append 出来的、不是从 map 里遍历出来的，所以去掉 slices.Sort
// 不会让任何测试闪烁——只会安静地换一种顺序，四十次运行四十次绿。（这一条是拿
// map-order 变异扫全包时顺手扫出来的：另外两个排序各有 40/40 的测试接住，
// 这一个是 0/40。）
//
// 而它现在是**承重**的：Rejected 超过 maxRejected 时是在排序**之后**截断的，
// "留下的是数值最小的那一批"这个确定性承诺全靠这一行。
func TestSubAckRejectedIsSortedToo(t *testing.T) {
	r := New()
	s := r.Add(SessionOpts{CID: "1000", MaxTX: 1, MaxRX: 1, Send: func([]byte) {}})
	// MaxTX 是 1，所以第一个之后的三个都被拒，且刻意不按升序声明。
	ack := r.Subscribe(s.ID, control.Sub{TX: []uint32{127800, 118000, 124550, 121800}})

	if len(ack.Rejected) != 3 {
		t.Fatalf("Rejected = %v, want the three over the MaxTX limit", ack.Rejected)
	}
	if !slices.IsSorted(ack.Rejected) {
		t.Fatalf("SubAck.Rejected = %v, want ascending order — the ACK's three list fields follow one contract, and the maxRejected truncation happens after this sort", ack.Rejected)
	}
}

// TestAFrequencyGrantedViaTxIsNotThenRejectedByTheRxLimit 钉住 RX 闸门里的
// already 判断。
//
// TX ⊆ RX，所以 TX 的频率先进 next.rx；RX 循环再看到同一个频率时 len(next.rx)
// 可能已经到顶。少了 already 判断，这个**已经授权**的频率会被追加进 Rejected，
// 于是它同时出现在 ack.RX 和 ack.Rejected 里——客户端同时被告知"给你了"和"拒了"。
func TestAFrequencyGrantedViaTxIsNotThenRejectedByTheRxLimit(t *testing.T) {
	r := New()
	s := r.Add(SessionOpts{CID: "1000", MaxTX: 2, MaxRX: 1, Send: func([]byte) {}})
	ack := r.Subscribe(s.ID, control.Sub{
		TX: []uint32{118000, 121800},
		RX: []uint32{118000},
	})

	for _, f := range ack.RX {
		for _, g := range ack.Rejected {
			if f == g {
				t.Fatalf("%d is in both RX %v and Rejected %v; it was granted through TX and must not then be bounced by the RX limit", f, ack.RX, ack.Rejected)
			}
		}
	}
	if !r.MayTransmit(s.ID, 118000) {
		t.Fatal("118000 was accepted for TX; MayTransmit must agree with the ACK")
	}
}

// TestSubscribeCopiesTheCrossCoupleList 确认 router 不会把调用方的切片存下来。
// 存下来的话，控制面那边复用或修改缓冲区就会改到服务端的路由状态。
//
// 存下来的具体后果在最后一段：会话保留 subs.xc 是为了下一次 Subscribe 知道该从
// 引用计数索引里摘掉哪些对。存的是别名的话，撤销时去减的是调用方改成的**那一对**，
// 121800↔124550 的计数永远减不掉——两个频率从此被永久接通。
func TestSubscribeCopiesTheCrossCoupleList(t *testing.T) {
	r := New()
	s := newSession(t, r, "1000", 8)
	xc := [][2]uint32{{121800, 124550}}
	r.Subscribe(s.ID, control.Sub{TX: []uint32{121800, 124550}, XC: xc})
	if got := r.coupledWith(121800); len(got) != 1 || got[0] != 124550 {
		t.Fatalf("coupledWith = %v, want [124550] — the coupling did not take effect at all", got)
	}

	xc[0] = [2]uint32{999000, 999001} // 调用方改自己的切片

	if got := r.coupledWith(121800); len(got) != 1 || got[0] != 124550 {
		t.Fatalf("coupledWith = %v, want [124550] — Subscribe aliased the caller's slice instead of copying it", got)
	}
	r.Subscribe(s.ID, control.Sub{TX: []uint32{121800, 124550}})
	if got := r.coupledWith(121800); len(got) != 0 {
		t.Fatalf("coupledWith = %v, want empty — the undo decremented whatever the caller's slice says now, not the pair actually indexed", got)
	}
}

// TestSubscribeRejectsRxBeyondMaxRX 钉住 RX 上限真的被强制。
//
// Task 9 的 Config 里有 MaxRX，而且在 READY 里告诉了客户端，但如果这里不拦，
// 那个数字就只是一句建议。每个 RX 频率都要写进 Router 的倒排索引，而那是在
// 写锁里做的——一个声明了一万个频率的会话会让全网的扇出排队等它。
func TestSubscribeRejectsRxBeyondMaxRX(t *testing.T) {
	r := New()
	s := r.Add(SessionOpts{CID: "1000", MaxTX: 8, MaxRX: 2, Send: func([]byte) {}})
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
	s := r.Add(SessionOpts{CID: "1000", MaxTX: 2, MaxRX: 2, Send: func([]byte) {}})
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
	if got := r.coupledWith(118000); len(got) != 0 {
		t.Fatalf("coupledWith = %v, want empty", got)
	}
	if r.MayTransmit(s.ID, 118000) {
		t.Fatal("a session that has not subscribed must not be able to transmit")
	}
	if len(r.Listeners(118000)) != 0 {
		t.Fatal("a session that has not subscribed must not be a listener")
	}
}

// TestASecondLoginOnTheSameCidEvictsTheFirst 是本任务的核心。
//
// 不顶号的话，半开连接掉线重连的成员会有最长 60 秒（QUIC 的 MaxIdleTimeout）两条
// 会话同时订阅同一批频率。他一说话，扇出送给"除自己以外的每个订阅者"——而"自己"
// 判的是 SessionID 不是 CID，旧会话不是这条新会话，于是它收到了，客户端把他自己的
// 声音播了出来。只在丢线重连之后出现，本地测不出来。
func TestASecondLoginOnTheSameCidEvictsTheFirst(t *testing.T) {
	r := New()
	closed := make(chan SessionID, 4)

	first := r.Add(SessionOpts{
		CID: "1000", MaxTX: 8, MaxRX: 64,
		Send:  func([]byte) {},
		Close: func() { closed <- 1 },
	})
	r.Subscribe(first.ID, control.Sub{RX: []uint32{118000}, TX: []uint32{118000}})
	if len(r.Listeners(118000)) != 1 {
		t.Fatal("the first session did not subscribe")
	}

	second := r.Add(SessionOpts{
		CID: "1000", MaxTX: 8, MaxRX: 64,
		Send:  func([]byte) {},
		Close: func() { closed <- 2 },
	})
	if second.ID == first.ID {
		t.Fatal("the second login must get its own session id")
	}

	// 旧会话必须不在了：既不在会话表里，也不在任何频率的订阅者集合里。
	if _, ok := r.Get(first.ID); ok {
		t.Fatal("the first session is still registered after the same cid logged in again")
	}
	if got := len(r.Listeners(118000)); got != 0 {
		t.Fatalf("118000 still has %d listeners; the evicted session must leave the inverted index", got)
	}
	if r.MayTransmit(first.ID, 118000) {
		t.Fatal("an evicted session must not be able to transmit")
	}

	// 而且它的连接必须被真的断开，否则客户端会以为自己还在线。
	select {
	case id := <-closed:
		if id != 1 {
			t.Fatalf("Close fired for session %d, want the evicted first one", id)
		}
	default:
		t.Fatal("the evicted session's Close callback was never called; its connection is still open and it will keep hearing traffic")
	}

	// 新会话完好。
	r.Subscribe(second.ID, control.Sub{RX: []uint32{118000}})
	if len(r.Listeners(118000)) != 1 {
		t.Fatal("the surviving session must still be able to subscribe")
	}
}

// TestEvictionDoesNotTouchOtherCids 确认顶号只顶自己那一条。
func TestEvictionDoesNotTouchOtherCids(t *testing.T) {
	r := New()
	other := newSession(t, r, "2000", 8)
	r.Subscribe(other.ID, control.Sub{RX: []uint32{118000}})

	_ = newSession(t, r, "1000", 8)
	_ = newSession(t, r, "1000", 8) // 顶掉上一条

	if _, ok := r.Get(other.ID); !ok {
		t.Fatal("a different cid's session was evicted")
	}
	if len(r.Listeners(118000)) != 1 {
		t.Fatal("a different cid's subscription was disturbed")
	}
}

// TestRemoveClearsTheCidIndex 钉住正常下线也要清 cid 索引。
// 不清的话，索引里留着一个已注销的 id；下一次同 cid 登录会对着它调 Close，
// 而那个闭包捕获的是一条已经关掉的连接。
func TestRemoveClearsTheCidIndex(t *testing.T) {
	r := New()
	var closes int
	first := r.Add(SessionOpts{
		CID: "1000", MaxTX: 8, MaxRX: 64,
		Send:  func([]byte) {},
		Close: func() { closes++ },
	})
	r.Remove(first.ID)
	if closes != 0 {
		t.Fatal("an ordinary Remove must not call Close — the transport is already tearing that connection down, and calling back into it invites a loop")
	}

	// 直接断言索引本身。这是包内测试，读 r.byCID 是正当的——而且这里
	// 没有别的办法：从外部看，"索引里留着一条指向已注销会话的记录"和
	// "索引是干净的"表现完全一样，因为 Add 的顶号路径查到的 session
	// 本来就已经不在 r.sessions 里了。
	if len(r.byCID) != 0 {
		t.Fatalf("byCID has %d entries after the only session was removed, want 0", len(r.byCID))
	}
}

// TestRepeatedConnectDisconnectDoesNotGrowTheCidIndex 是上一条真正的价值所在。
//
// byCID 不清理的话，每一个不同的成员连接/断开一次，就会永久留下一条记录。
// 一台长期运行的服务器上，那是一条只增不减的索引——从外部完全看不出来，
// 直到内存出问题。每次循环必须用不同的 CID：同一个 CID 重复登录只会覆写
// map 里的同一个 key，测不出"从外部看不出来的累积"这件事。
func TestRepeatedConnectDisconnectDoesNotGrowTheCidIndex(t *testing.T) {
	r := New()
	for i := 0; i < 1000; i++ {
		cid := fmt.Sprintf("%d", 1000+i)
		s := r.Add(SessionOpts{CID: cid, MaxTX: 8, MaxRX: 64, Send: func([]byte) {}})
		r.Subscribe(s.ID, control.Sub{RX: []uint32{118000}})
		r.Remove(s.ID)
	}
	if len(r.byCID) != 0 {
		t.Fatalf("byCID has %d entries after 1000 distinct members connected and disconnected, want 0", len(r.byCID))
	}
	if len(r.sessions) != 0 {
		t.Fatalf("sessions has %d entries after 1000 distinct members connected and disconnected, want 0", len(r.sessions))
	}
	if len(r.rx) != 0 {
		t.Fatalf("rx has %d frequency entries after 1000 distinct members connected and disconnected, want 0", len(r.rx))
	}
}

// TestEvictionIsSafeWithoutACloseCallback 确认 Close 可以不给。
// 测试里到处都是只给 Send 的会话，nil 回调不能让 Add panic。
func TestEvictionIsSafeWithoutACloseCallback(t *testing.T) {
	r := New()
	_ = r.Add(SessionOpts{CID: "1000", MaxTX: 8, MaxRX: 64, Send: func([]byte) {}})
	_ = r.Add(SessionOpts{CID: "1000", MaxTX: 8, MaxRX: 64, Send: func([]byte) {}})
	// 走到这里没 panic 就算过。
}

// TestAnEmptyCidIsNotEvictable 确认空 CID 不会互相顶。
// 空 CID 不该出现（鉴权拒绝空 CID），但如果真出现了，让所有空 CID 会话互相顶号
// 是比放过去糟糕得多的失败模式。
func TestAnEmptyCidIsNotEvictable(t *testing.T) {
	r := New()
	a := r.Add(SessionOpts{CID: "", MaxTX: 8, MaxRX: 64, Send: func([]byte) {}})
	b := r.Add(SessionOpts{CID: "", MaxTX: 8, MaxRX: 64, Send: func([]byte) {}})
	if _, ok := r.Get(a.ID); !ok {
		t.Fatal("a session with an empty cid was evicted by another empty-cid session")
	}
	if _, ok := r.Get(b.ID); !ok {
		t.Fatal("the second empty-cid session is missing")
	}
}

// TestConcurrentLoginsOnOneCidLeaveExactlyOneSurvivor 在 -race 下跑。
// 重连风暴里同一个 CID 短时间内连上好几次是真实存在的。
func TestConcurrentLoginsOnOneCidLeaveExactlyOneSurvivor(t *testing.T) {
	r := New()
	var wg sync.WaitGroup
	ids := make(chan SessionID, 16)
	for i := 0; i < 16; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			s := r.Add(SessionOpts{CID: "1000", MaxTX: 8, MaxRX: 64, Send: func([]byte) {}})
			ids <- s.ID
		}()
	}
	wg.Wait()
	close(ids)

	var alive int
	for id := range ids {
		if _, ok := r.Get(id); ok {
			alive++
		}
	}
	if alive != 1 {
		t.Fatalf("%d sessions survived 16 concurrent logins on one cid, want exactly 1", alive)
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

// TestTruncateDeclarationBoundsWhatItKeepsAndWhatItReports 在**它自己那一层**
// 钉住截断。
//
// 必须在这一层钉，因为从 Subscribe 的出口看不见它：超出上界的频率反正也会被
// MaxRX/MaxTX 拒，而超出的部分同样进 Rejected，所以"截了"和"没截"给出的 ACK
// 一模一样。两道闸执行同一条策略、互相遮蔽——正是本仓库那条"每道闸要在它自己
// 那一层被钉住"说的情形。它们唯一的区别是**干了多少活**，而那由下面那条
// TestAnOverlongDeclarationDoesNotAllocateInProportionToIt 量。
func TestTruncateDeclarationBoundsWhatItKeepsAndWhatItReports(t *testing.T) {
	xs := make([]uint32, 0, 5000)
	for i := 0; i < 5000; i++ {
		xs = append(xs, uint32(i))
	}

	kept, excess, unreported := truncateDeclaration(xs, 128)
	if len(kept) != 128 {
		t.Fatalf("kept %d, want the first 128", len(kept))
	}
	if kept[0] != 0 || kept[127] != 127 {
		t.Fatalf("kept = %v…%v, want the declaration order preserved", kept[0], kept[127])
	}
	// 回报也要有界，理由和 normaliseXC 的上界分支一样：整份抄回去会让 SUBACK
	// 超过 64 KiB 的帧上限而根本发不出去，客户端于是什么都收不到。
	if len(excess) != maxRejected {
		t.Fatalf("excess = %d entries, want it capped at maxRejected (%d) — copying all %d back makes the ACK too large to send at all", len(excess), maxRejected, len(xs)-128)
	}
	if excess[0] != 128 {
		t.Fatalf("excess starts at %d, want the first entry past the limit (128)", excess[0])
	}
	// 装不下的那一截要**数出来**。截断是对的，"截断了却不说"不是——那正是
	// 客户端拿着一份看起来完全正常的 ACK、却少了几百条拒绝的那种失败。
	if want := len(xs) - 128 - maxRejected; unreported != want {
		t.Fatalf("unreported = %d, want %d — the entries that fit in neither the kept list nor the excess list are exactly the ones nothing else will ever mention", unreported, want)
	}

	// 对照：没超界的声明必须原样通过，而且**不报**任何超出。一个无条件截断的
	// 实现光靠上面那半是抓不住的。
	short := xs[:10]
	kept, excess, unreported = truncateDeclaration(short, 128)
	if len(kept) != 10 || len(excess) != 0 || unreported != 0 {
		t.Fatalf("a declaration inside the limit came back as %d kept / %d excess / %d unreported, want 10 / 0 / 0", len(kept), len(excess), unreported)
	}
}

// TestTheDeclarationLimitFollowsTheSessionsOwnCeilings 钉住上界是从会话算出来的，
// 不是一个写死的数。
//
// 写死的话，一个 MaxRX 配得很大的部署会在完全正当的声明上被截断——症状是
// 管制员台面上靠后的几个频率安静地不生效。
func TestTheDeclarationLimitFollowsTheSessionsOwnCeilings(t *testing.T) {
	r := New()
	s := r.Add(SessionOpts{CID: "1000", MaxTX: 4, MaxRX: 100, Send: func([]byte) {}})
	limit, ok := r.declarationLimit(s.ID)
	if !ok {
		t.Fatal("declarationLimit did not find a session that was just added")
	}
	if want := 100 * declaredSlack; limit != want {
		t.Fatalf("limit = %d, want %d — the bound is slack × max(MaxTX, MaxRX), so the larger ceiling wins", limit, want)
	}

	// TX 那一侧也要算进去：MaxTX 比 MaxRX 大时（router 允许这种组合），
	// 按 MaxRX 算会把合法的 TX 声明截掉。
	s2 := r.Add(SessionOpts{CID: "1001", MaxTX: 50, MaxRX: 8, Send: func([]byte) {}})
	limit, _ = r.declarationLimit(s2.ID)
	if want := 50 * declaredSlack; limit != want {
		t.Fatalf("limit = %d, want %d", limit, want)
	}

	if _, ok := r.declarationLimit(SessionID(999999)); ok {
		t.Fatal("declarationLimit claims to know an unknown session")
	}
}

// TestAnOverlongDeclarationDoesNotAllocateInProportionToIt 是声明上界真正要防的
// 那件事的**确定性**度量。
//
// 缺陷是这样的：Subscribe 在全服务端那一把写锁里处理客户端声明了多少就是多少
// 的频率。实测一份塞满的 SUB 装 9356 个频率（65533 字节），一次调用持 r.mu
// 280 微秒，同一时间 50 次 Listeners() 从 11 微秒被推到 67 毫秒——6039 倍。
//
// 为什么量分配字节而不是量时间：时间断言在一台忙机器上、在 GOMAXPROCS=1 下
// （那里根本不会发生并行等待）要么假绿要么闪烁，本仓库明令"概率性的钉子不算
// 钉子"。分配量是同一件事的确定性投影——干 O(n) 的活就要为 n 建去重 map。
//
// 量出来的数（Go 1.27，-race 与非 -race、GOMAXPROCS=1 与默认，四种组合各三次，
// 全部相同）：有截断 32776 字节，去掉截断 1151104 字节，相差 35 倍。上限取
// 128 KiB，两边各留约 4 倍和 9 倍余量。
func TestAnOverlongDeclarationDoesNotAllocateInProportionToIt(t *testing.T) {
	const declared = 9356 // 一条 64 KiB 的 SUB 塞得下的最多频率数（实测）
	const budget = 128 << 10

	freqs := make([]uint32, 0, declared)
	for i := 0; i < declared; i++ {
		freqs = append(freqs, uint32(i))
	}
	r := New()
	s := r.Add(SessionOpts{CID: "1000", MaxTX: 8, MaxRX: 32, Send: func([]byte) {}})
	// 先跑一次小的：把首次调用里那些一次性的分配（map 的初始桶之类）挪到
	// 测量窗口之外，否则它们会被算到这一份声明头上。
	r.Subscribe(s.ID, control.Sub{RX: []uint32{118000}})

	var before, after runtime.MemStats
	runtime.GC()
	runtime.ReadMemStats(&before)
	ack := r.Subscribe(s.ID, control.Sub{RX: freqs, TX: freqs})
	runtime.ReadMemStats(&after)

	if got := after.TotalAlloc - before.TotalAlloc; got > budget {
		t.Fatalf("one SUB declaring %d frequencies allocated %d bytes, over the %d byte budget — the declaration is being processed in full inside the router's global write lock, and every fan-out on the server waits behind it",
			declared, got, budget)
	}
	// 前提：这份声明确实被处理了，而不是被整个丢掉。
	if len(ack.RX) != 32 {
		t.Fatalf("accepted RX = %d, want MaxRX (32) — if the declaration were simply dropped the budget above would pass for the wrong reason", len(ack.RX))
	}
	// 而且超出上界的部分不是静默消失的。
	if len(ack.Rejected) == 0 {
		t.Fatal("nothing was reported as rejected; a client that declared 9356 frequencies and got 32 must be told something")
	}
	if len(ack.Rejected) > maxRejected {
		t.Fatalf("Rejected = %d entries, want at most %d", len(ack.Rejected), maxRejected)
	}
}

// TestATruncatedRejectionListSaysSoOnTheWire 钉住 SubAck.RejectedTruncated。
//
// 实测的形状：MaxRX=32 的会话声明 1000 个频率。两道上界依次动手——
// declarationLimit 把 1000 截到 128（并且只把其中 256 个的超出部分抄进回报，
// 剩下 616 个连抄都没抄），maxRejected 又把最终的 352 条拒绝砍到 256。
// 于是 **712 条拒绝无标记、无日志地消失**，而客户端拿到的 ACK 看起来完全正常：
// 它没有任何办法知道这份清单是不全的。
//
// 截断本身是对的（不设界的 ACK 超过 64 KiB 就根本发不出去，客户端什么都收不到，
// 那更糟）。要改的是"不说"。
func TestATruncatedRejectionListSaysSoOnTheWire(t *testing.T) {
	r := New()
	s := r.Add(SessionOpts{CID: "1000", MaxTX: 8, MaxRX: 32, Send: func([]byte) {}})

	rx := make([]uint32, 0, 1000)
	for i := 0; i < 1000; i++ {
		rx = append(rx, uint32(118000+i))
	}
	ack := r.Subscribe(s.ID, control.Sub{RX: rx})

	if len(ack.Rejected) != maxRejected {
		t.Fatalf("Rejected has %d entries, want it capped at maxRejected (%d) — the premise of this test is that the list really did overflow", len(ack.Rejected), maxRejected)
	}
	if !ack.RejectedTruncated {
		lost := len(rx) - len(ack.RX) - len(ack.Rejected)
		t.Fatalf("RejectedTruncated is false while %d of the %d declared frequencies are in neither ack.RX nor ack.Rejected; the client has no way to tell this list is incomplete", lost, len(rx))
	}

	// 客户端拿到这个标记之后该走的那条路：差集。它任何时候都成立，而 Rejected
	// 只是"拒了三两个"这种常见情况下的便利字段。这里顺带证明差集确实补得回来。
	accepted := map[uint32]struct{}{}
	for _, f := range ack.RX {
		accepted[f] = struct{}{}
	}
	for _, f := range ack.TX {
		accepted[f] = struct{}{}
	}
	missing := 0
	for _, f := range rx {
		if _, ok := accepted[f]; !ok {
			missing++
		}
	}
	if want := len(rx) - len(ack.RX); missing != want {
		t.Fatalf("the difference set recovers %d rejected frequencies, want %d", missing, want)
	}
}

// TestAnOrdinaryPartialRejectionIsNotMarkedTruncated 是上一条的反例。
//
// 没有它，一个无条件把 RejectedTruncated 设成 true 的实现照样全绿——而那样
// 客户端每收到一份 ACK 都要去算差集，等于把这个标记变回噪音。
func TestAnOrdinaryPartialRejectionIsNotMarkedTruncated(t *testing.T) {
	r := New()
	s := r.Add(SessionOpts{CID: "1000", MaxTX: 2, MaxRX: 4, Send: func([]byte) {}})

	ack := r.Subscribe(s.ID, control.Sub{RX: []uint32{118000, 118100, 118200, 118300, 118400, 118500}})
	if len(ack.Rejected) != 2 {
		t.Fatalf("Rejected = %v, want the two frequencies past MaxRX=4 — the premise is that something really was rejected", ack.Rejected)
	}
	if ack.RejectedTruncated {
		t.Fatal("RejectedTruncated is true for an ACK that lists every rejected frequency; the flag means \"this list is incomplete\", and a client that sees it on every ACK will stop reading it")
	}
}
