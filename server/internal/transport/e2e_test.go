package transport

import (
	"context"
	"crypto/ed25519"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/control"
	"github.com/JianyueLab-Org/can-voice/server/internal/fsdfeed"
	"github.com/JianyueLab-Org/can-voice/server/internal/geo"
	"github.com/JianyueLab-Org/can-voice/server/internal/router"
	"github.com/JianyueLab-Org/can-voice/server/internal/wire"
)

// 本文件是整支栈唯一一处**按生产的方式接线之后**再跑的测试。
//
// 判据是：一个缺陷如果能在某个包的单元测试里表现出来，它就不属于这里。
// 所以"一个人说话另一个人听见"（pump_test.go）、顶号的关闭码（conn_test.go）、
// 交叉耦合的路由（router/fanout_test.go）、射程的数学（geo/）、SSE 的解析
// （fsdfeed/feed_test.go）都不在这里重写。剩下的只有部件之间的接线本身。
//
// client / connect / subscribe 在 helpers_test.go，testServer / dial / hello 在
// conn_test.go——同一个包里第二份定义编译不过，所以这里只新增
// testServerWithFeed、waitFor 和两个收包小工具。

// TestTheWholeStackAttenuatesByDistance 是本任务存在的理由。
//
// 射程衰减的三块在三个包里各自都被测过，但从来没有在同一个进程里连起来跑过：
// router 的测试用桩 locator，fsdfeed 的测试碰不到 router，transport 的测试不装
// locator。这里把真实的 Feed（读真实格式的 SSE）接到真实的 Router 上，
// 经真实的 QUIC 连接收包。
//
// 几何（按今天的 geo 常量，不要"简化"这几个纬度）：
//
//	LOS(35000 ft) = 1.23×√35000 = 230.112 NM，两架飞机相加得射程 460.224 NM
//	满格 ≤ 0.8×460.224 = 368.18 NM，截止 1.1×460.224 = 506.25 NM
//	一个纬度 = 3440.065×π/180 = 60.0405 NM（大圆，不是恰好 60）
//
//	near  Δlat=1.6667  d=100.07 NM  ratio=0.2174  → 255
//	mid   Δlat=7.3333  d=440.29 NM  ratio=0.9567  → 衰减带里
//	far   Δlat=11.6667 d=700.47 NM  ratio=1.5220  → 不投递
//
// 上面这几个数是**注释**，下面每一条失败信息里的数字都是当场按 geo 的常量算的。
// 这不是洁癖：这个测试对 geo.FullRatio/CutoffRatio 有隐含耦合（把 FullRatio 提到
// 0.9567 以上，mid 就落进满格带），而 grep 那两个常量只会找到 geo 自己——改它们
// 的人不会被任何东西提醒。写死的"368–506"在那一刻会变成一句**假话**，
// 把下一个人送去查射程数学或者 feed，而真正动的是两个比值。
//
// 中间那一档的具体数值（今天是 122）**刻意不写进断言**：它依赖衰减曲线的形状，
// 而那条曲线是可以调的。要钉的性质是"严格在 0 和 255 之间，且严格低于近的那个"。
func TestTheWholeStackAttenuatesByDistance(t *testing.T) {
	const snapshot = `{"general":{"version":"CAN BETA TEST"},"pilots":[` +
		`{"callsign":"CCA1000","cid":"1000","latitude":30.0,"longitude":120.0,"altitude":35000},` +
		`{"callsign":"CCA1001","cid":"1001","latitude":31.6667,"longitude":120.0,"altitude":35000},` +
		`{"callsign":"CCA1002","cid":"1002","latitude":37.3333,"longitude":120.0,"altitude":35000},` +
		`{"callsign":"CCA1003","cid":"1003","latitude":41.6667,"longitude":120.0,"altitude":35000}` +
		`],"controllers":[],"atis":[]}`

	// 全部按 geo 的常量当场算，失败信息里报的就是这几个数。
	const cruiseFt = 35000.0
	rangeNM := 2 * geo.LOSTermNM(cruiseFt) // 两架同高度的飞机：两个半项相加
	fullNM := geo.FullRatio * rangeNM
	cutoffNM := geo.CutoffRatio * rangeNM
	nearNM := geo.DistanceNM(30.0, 120.0, 31.6667, 120.0)
	midNM := geo.DistanceNM(30.0, 120.0, 37.3333, 120.0)
	farNM := geo.DistanceNM(30.0, 120.0, 41.6667, 120.0)

	// 前提：这三个纬度是按今天的 FullRatio/CutoffRatio 选的。谁改了那两个常量，
	// 本测试要红在**这里**并说出原因，而不是红在"mid 拿了 255"上。
	if nearNM >= fullNM {
		t.Fatalf("premise: near is %.1f NM but full strength only reaches %.1f NM (%.2f × %.1f) — geo.FullRatio moved and these latitudes no longer mean what the test needs", nearNM, fullNM, geo.FullRatio, rangeNM)
	}
	if midNM <= fullNM || midNM >= cutoffNM {
		t.Fatalf("premise: mid is %.1f NM, which is not strictly inside the %.1f–%.1f taper (%.2f–%.2f × %.1f) — geo.FullRatio/CutoffRatio moved and these latitudes no longer mean what the test needs", midNM, fullNM, cutoffNM, geo.FullRatio, geo.CutoffRatio, rangeNM)
	}
	if farNM <= cutoffNM {
		t.Fatalf("premise: far is %.1f NM but the cutoff is %.1f NM (%.2f × %.1f) — geo.CutoffRatio moved and these latitudes no longer mean what the test needs", farNM, cutoffNM, geo.CutoffRatio, rangeNM)
	}

	addr, priv, _ := testServerWithFeed(t, snapshot)

	speaker := connect(t, addr, "1000", 8, priv)
	near := connect(t, addr, "1001", 8, priv)
	mid := connect(t, addr, "1002", 8, priv)
	far := connect(t, addr, "1003", 8, priv)

	// subscribe 自己会把"被拒"变成 Fatal，所以下面"听不见"的断言不会因为
	// 订阅其实没生效而假绿。
	speaker.subscribe(t, control.Sub{TX: []uint32{121800}})
	for _, c := range []*client{near, mid, far} {
		c.subscribe(t, control.Sub{RX: []uint32{121800}})
	}

	pkt := append(wire.Header{
		Ver: wire.Version, Flags: wire.FlagFirst, Seq: 1, FreqKHz: 121800,
	}.AppendTo(nil), 0x01, 0x02, 0x03, 0x04)
	if err := speaker.conn.SendDatagram(pkt); err != nil {
		t.Fatalf("SendDatagram: %v", err)
	}

	nearQual := qualityOf(t, near, 3*time.Second)
	midQual := qualityOf(t, mid, 3*time.Second)

	if nearQual != 255 {
		t.Fatalf("near Qual = %d, want 255 — %.1f NM is well inside the full-strength radius of %.1f NM (%.2f × %.1f)", nearQual, nearNM, fullNM, geo.FullRatio, rangeNM)
	}
	if midQual == 0 || midQual == 255 {
		t.Fatalf("mid Qual = %d, want something strictly between 0 and 255 — %.1f NM is inside the %.1f–%.1f taper", midQual, midNM, fullNM, cutoffNM)
	}
	// 这一句今天**不可能失败**：上面两句已经把 nearQual 钉成 255、把 midQual 钉进
	// [1, 254]。留着不是为了覆盖，是因为它是三句里唯一一句直接说出本测试主旨的话
	// ——"随距离衰减"——而且一旦将来有人把 near 那句放宽成一个区间，它立刻接管。
	if midQual >= nearQual {
		t.Fatalf("mid Qual %d is not below near Qual %d; the whole point is that it attenuates with distance", midQual, nearQual)
	}

	// 远的那个什么都不该收到。这是一条否定式断言，而它的前提由上面两句
	// 保证了：包确实发出去了，而且确实扇出给了别人。扇出是同一个 goroutine
	// 里的一次串行遍历，所以 near/mid 已经拿到包之后，这半秒只需要盖住
	// 同一轮扇出里两次投递之间的间隔。
	ctx, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
	defer cancel()
	if _, err := far.conn.ReceiveDatagram(ctx); err == nil {
		t.Fatalf("a listener %.1f NM away must hear nothing — the cutoff is %.1f NM (%.2f × %.1f)", farNM, cutoffNM, geo.CutoffRatio, rangeNM)
	}
}

// TestResendingSubAfterAReconnectRestoresEverything 钉住声明式订阅的核心价值：
// 重连之后客户端只需重发**同一份**全量声明，不需要任何"我原来在哪个频道"的记忆，
// 也没有任何增量入口要它去推导。
func TestResendingSubAfterAReconnectRestoresEverything(t *testing.T) {
	addr, priv, r := testServer(t)

	// 同一个变量用两次，一字未改——"重发同一份声明"是这条测试要证的东西，
	// 各写一份字面量会把它变成"重新声明一次"。
	sub := control.Sub{RX: []uint32{118000, 121800}}

	first := connect(t, addr, "1000", 8, priv)
	first.subscribe(t, sub)
	if got := len(r.Listeners(118000)); got != 1 {
		t.Fatalf("premise: Listeners(118000) = %d after the first SUB, want 1", got)
	}
	if got := len(r.Listeners(121800)); got != 1 {
		t.Fatalf("premise: Listeners(121800) = %d after the first SUB, want 1", got)
	}

	first.conn.CloseWithError(CloseNormal, "simulated drop")

	// 必须先等旧会话真的被摘掉，而且这不是为了消除竞态——是为了让下面那句
	// "回到 1"有内容。顶号会顺手做到同一个计数：Router.Add 摘掉同 CID 的旧
	// 会话，于是就算断连路径整个坏掉，重连之后两个频率照样各是 1，测试测不到
	// 重发 SUB 有没有恢复任何东西。
	waitFor(t, 3*time.Second, func() bool {
		return len(r.Listeners(118000)) == 0 && len(r.Listeners(121800)) == 0
	}, "the dropped session is still indexed as a listener; the reconnect below would have been measuring eviction, not restoration")

	second := connect(t, addr, "1000", 8, priv)
	second.subscribe(t, sub)

	if got := len(r.Listeners(118000)); got != 1 {
		t.Fatalf("Listeners(118000) = %d after resending the same SUB, want 1", got)
	}
	if got := len(r.Listeners(121800)); got != 1 {
		t.Fatalf("Listeners(121800) = %d after resending the same SUB, want 1", got)
	}
}

// TestAListenerOnAnotherFrequencyHearsNothing 钉住频率隔离。
//
// 这是一条否定式断言，所以它自带前提的证明：场上有一个 witness 订阅的正是
// 发射的那个频率，他必须先收到，"124550 上的人听不到"才是一句有内容的话。
func TestAListenerOnAnotherFrequencyHearsNothing(t *testing.T) {
	addr, priv, _ := testServer(t)

	speaker := connect(t, addr, "1000", 8, priv)
	witness := connect(t, addr, "1001", 8, priv)
	other := connect(t, addr, "1002", 8, priv)

	speaker.subscribe(t, control.Sub{TX: []uint32{121800}})
	witness.subscribe(t, control.Sub{RX: []uint32{121800}})
	other.subscribe(t, control.Sub{RX: []uint32{124550}})

	pkt := append(wire.Header{
		Ver: wire.Version, Flags: wire.FlagFirst, Seq: 3, FreqKHz: 121800,
	}.AppendTo(nil), 0xaa, 0xbb)
	if err := speaker.conn.SendDatagram(pkt); err != nil {
		t.Fatalf("SendDatagram: %v", err)
	}

	// 前提：这一包确实被扇出了。
	h := receivedHeader(t, witness, 3*time.Second, "the witness on the transmitted frequency")
	if h.FreqKHz != 121800 {
		t.Fatalf("the witness got FreqKHz = %d, want 121800", h.FreqKHz)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
	defer cancel()
	if _, err := other.conn.ReceiveDatagram(ctx); err == nil {
		t.Fatal("a listener subscribed only to 124550 must not hear 121800")
	}
}

// qualityOf 收一个包并返回包头上的 Qual。
func qualityOf(t *testing.T, c *client, limit time.Duration) uint8 {
	t.Helper()
	return receivedHeader(t, c, limit, "the listener").Qual
}

// receivedHeader 收一个包并解出包头。who 只进失败信息，用来分辨是哪一个听众
// 没收到——三个听众轮流调它，一条不带主语的失败信息说不清是哪一条红了。
func receivedHeader(t *testing.T, c *client, limit time.Duration, who string) wire.Header {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), limit)
	defer cancel()
	got, err := c.conn.ReceiveDatagram(ctx)
	if err != nil {
		t.Fatalf("%s (session %d) received nothing within %s: %v", who, c.sess, limit, err)
	}
	h, _, err := wire.Parse(got)
	if err != nil {
		t.Fatalf("Parse: %v", err)
	}
	return h
}

// testServerWithFeed 起一个接了真实 fsdfeed.Feed 的服务端，Feed 指向一个
// 只发一次快照、然后一直握住连接不放的假 SSE 服务端。
//
// **必须握住不放**：一发完就返回的话，那是一次优雅关闭，Feed 会照设计降级
// （fsdfeed 里有专门的测试钉这条），而降级的语义是"全部满格放行"——
// 于是这个测试里的每个人都拿 255，它就再也测不到任何东西了。
func testServerWithFeed(t *testing.T, snapshot string) (addr string, priv ed25519.PrivateKey, r *router.Router) {
	t.Helper()
	sse := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		fl, ok := w.(http.Flusher)
		if !ok {
			t.Error("the test server does not support flushing")
			return
		}
		fmt.Fprintf(w, "event: snapshot\ndata: %s\n\n", snapshot)
		fl.Flush()
		<-req.Context().Done()
	}))
	t.Cleanup(sse.Close)

	addr, priv, r = testServer(t)
	feed := fsdfeed.NewFeed(sse.URL)
	ctx, cancel := context.WithCancel(context.Background())
	// 这条 cleanup 注册在 sse.Close 之后，所以（LIFO）它先跑：先让 Feed 松开
	// 那条 SSE 连接，httptest 的 Close 才等得到它的 handler 返回。
	t.Cleanup(cancel)
	go feed.Run(ctx)
	r.SetLocator(feed)

	// 等快照真的到位再返回。不等的话，前几个测试动作会在一个空快照上跑，
	// 而空快照的语义是"位置未知 → 不衰减"，于是所有人都拿 255。
	waitFor(t, 3*time.Second, func() bool {
		return len(feed.Snapshot().ByCID) > 0 && !feed.Degraded()
	}, "the feed never delivered its snapshot")
	return addr, priv, r
}

// waitFor 轮询一个条件直到超时。端到端测试里每一处"等服务端处理完"都要有上界，
// 否则一个真实的挂起会表现为测试永远跑不完，而不是一条失败信息。
func waitFor(t *testing.T, limit time.Duration, cond func() bool, msg string) {
	t.Helper()
	deadline := time.Now().Add(limit)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatal(msg)
}
