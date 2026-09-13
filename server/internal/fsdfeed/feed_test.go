package fsdfeed

import (
	"context"
	"fmt"
	"io"
	"math"
	"net/http"
	"net/http/httptest"
	"os"
	"reflect"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/geo"
)

func sample(t *testing.T) Snapshot {
	t.Helper()
	b, err := os.ReadFile("../../testdata/datafeed_sample.json")
	if err != nil {
		t.Fatalf("read sample: %v", err)
	}
	s, err := ParseDatafeed(b)
	if err != nil {
		t.Fatalf("ParseDatafeed: %v", err)
	}
	return s
}

// 这是本包最重要的测试。can-fsd 的 datafeed 里飞行员的经纬度是 JSON 数字，
// 管制员和 ATIS 的是 JSON 字符串。只当数字解析的话所有管制员会落在 0,0
// （几内亚湾），表现为管制员谁都听不见，而日志里一条错误都没有。
func TestControllerCoordinatesParseFromStrings(t *testing.T) {
	s := sample(t)
	p, ok := s.ByCallsign["ZSHA_CTR"]
	if !ok {
		t.Fatal("ZSHA_CTR missing from the snapshot")
	}
	if math.Abs(p.Lat-31.20466) > 0.001 || math.Abs(p.Lon-121.45272) > 0.001 {
		t.Fatalf("ZSHA_CTR at %v,%v — controller coordinates are JSON strings and must still parse", p.Lat, p.Lon)
	}
	if p.Lat == 0 && p.Lon == 0 {
		t.Fatal("ZSHA_CTR landed at 0,0: the string/number asymmetry was not handled")
	}
}

func TestPilotCoordinatesParseFromNumbers(t *testing.T) {
	s := sample(t)
	p, ok := s.ByCallsign["CCA5852"]
	if !ok {
		t.Fatal("CCA5852 missing from the snapshot")
	}
	if math.Abs(p.Lat-25.10232) > 0.001 || math.Abs(p.Lon-102.933) > 0.001 {
		t.Fatalf("CCA5852 at %v,%v, want 25.10232,102.933", p.Lat, p.Lon)
	}
	if p.AltFt != 6897 {
		t.Fatalf("CCA5852 altitude = %v, want 6897", p.AltFt)
	}
}

func TestPilotRangeComesFromAltitudeNotVisualRange(t *testing.T) {
	// 飞行员的 visual_range 在样本里是 40，但 VHF 视距由高度决定：
	// 6897 英尺约 102 海里。用 visual_range 会把射程砍掉 60%。
	s := sample(t)
	p := s.ByCallsign["CCA5852"]
	want := geo.LOSTermNM(6897)
	if math.Abs(p.LOSTermNM-want) > 1e-9 {
		t.Fatalf("pilot LOSTermNM = %v, want %v (line of sight term from 6897 ft)", p.LOSTermNM, want)
	}
}

func TestControllerRangeUsesVisualRange(t *testing.T) {
	s := sample(t)
	if p := s.ByCallsign["ZSHA_CTR"]; p.RadiusNM != 600 {
		t.Fatalf("ZSHA_CTR RadiusNM = %v, want 600 from visual_range", p.RadiusNM)
	}
}

func TestZeroVisualRangeFallsBackToTheSuffixTable(t *testing.T) {
	// 样本里 ZSSS_ATIS 的 visual_range 正是 0。
	s := sample(t)
	p, ok := s.ByCallsign["ZSSS_ATIS"]
	if !ok {
		t.Fatal("ZSSS_ATIS missing from the snapshot")
	}
	if p.RadiusNM != 60 {
		t.Fatalf("ZSSS_ATIS RadiusNM = %v, want the 60 nm _ATIS fallback (its visual_range is 0)", p.RadiusNM)
	}
}

func TestSnapshotIsIndexedByCID(t *testing.T) {
	// cid 是关联键：token 里本来就有它，所以正常情况下协议里一个字段都不用加。
	s := sample(t)
	if _, ok := s.ByCID["1012"]; !ok {
		t.Fatal("snapshot must be indexed by cid; 1012 (CCA5852) is missing")
	}
}

func TestParseDatafeedRejectsGarbage(t *testing.T) {
	if _, err := ParseDatafeed([]byte("not json")); err == nil {
		t.Fatal("ParseDatafeed must reject non-JSON input")
	}
}

// realUpdateEvent 是 can-fsd 真正发出来的 update 形状：每个集合是一个带
// changed/removed 的对象，不是数组；缺席的集合整个键都不存在（omitempty）。
// 计划正文假设的是"数组 + 顶层 removed"，那个形状 can-fsd 从来没发过。
const realUpdateEvent = `{"update":1757000000,
	"pilots":{"changed":[
		{"callsign":"CCA1","cid":"1","latitude":30.5,"longitude":120.5,"altitude":11000}
	]}}`

// 保持单行：can-fsd 的 json.Marshal 输出本来就是单行的紧凑 JSON，而这个
// 常量还要经 fmt.Fprintf("data: %s\n\n", …) 直接上线——嵌入原始换行会把
// 后续物理行发成没有 "data:" 前缀的续行，被规范正确地当成未知字段丢弃，
// 表现为 JSON 从中间被截断（"unexpected end of JSON input"）。
const realSnapshotEvent = `{"general":{"version":"CAN BETA TEST"},"pilots":[{"callsign":"CCA1","cid":"1","latitude":30.0,"longitude":120.0,"altitude":10000},{"callsign":"CCA2","cid":"2","latitude":31.0,"longitude":121.0,"altitude":20000}],"controllers":[],"atis":[]}`

func TestUpdateEventUsesTheRealDeltaShape(t *testing.T) {
	// 这是本任务的核心测试。Task 5 的实现在这一条上会报
	// "cannot unmarshal object into Go struct field datafeed.pilots"。
	f := NewFeed("http://example.invalid")
	if err := f.applyEvent("snapshot", []byte(realSnapshotEvent)); err != nil {
		t.Fatalf("applyEvent snapshot: %v", err)
	}
	if err := f.applyEvent("update", []byte(realUpdateEvent)); err != nil {
		t.Fatalf("applyEvent update: %v — can-fsd's update envelope is feedDelta, not a datafeed document", err)
	}
	s := f.Snapshot()
	if len(s.ByCallsign) != 2 {
		t.Fatalf("after update: %d entries, want 2 (an update is a delta, not a replacement)", len(s.ByCallsign))
	}
	if p := s.ByCallsign["CCA1"]; p.Lat != 30.5 {
		t.Fatalf("CCA1 lat = %v, want the updated 30.5", p.Lat)
	}
	if _, ok := s.ByCallsign["CCA2"]; !ok {
		t.Fatal("CCA2 vanished after an update that did not mention it")
	}
}

func TestUpdateEventRemovesPerCollection(t *testing.T) {
	// removed 在每个集合里，不在顶层。
	f := NewFeed("http://example.invalid")
	if err := f.applyEvent("snapshot", []byte(realSnapshotEvent)); err != nil {
		t.Fatalf("applyEvent snapshot: %v", err)
	}
	if err := f.applyEvent("update", []byte(`{"update":2,"pilots":{"removed":["CCA2"]}}`)); err != nil {
		t.Fatalf("applyEvent update: %v", err)
	}
	s := f.Snapshot()
	if _, ok := s.ByCallsign["CCA2"]; ok {
		t.Fatal("CCA2 was listed as removed and must be gone")
	}
	if _, ok := s.ByCID["2"]; ok {
		t.Fatal("a removed callsign must also leave the cid index")
	}
	if _, ok := s.ByCallsign["CCA1"]; !ok {
		t.Fatal("CCA1 must survive an update that removed someone else")
	}
}

func TestAnEmptyUpdateChangesNothing(t *testing.T) {
	// 安静的网络：can-fsd 的每个集合都是 omitempty 指针，所以一个什么都没
	// 发生的 tick 发出来就是 {"update":N}。这不能被当成"全网清空"。
	f := NewFeed("http://example.invalid")
	if err := f.applyEvent("snapshot", []byte(realSnapshotEvent)); err != nil {
		t.Fatalf("applyEvent snapshot: %v", err)
	}
	before := f.Snapshot()
	if err := f.applyEvent("update", []byte(`{"update":3}`)); err != nil {
		t.Fatalf("applyEvent update: %v", err)
	}
	if got := len(f.Snapshot().ByCallsign); got != len(before.ByCallsign) {
		t.Fatalf("an update with no collections changed the snapshot from %d to %d entries",
			len(before.ByCallsign), got)
	}
}

func TestControllerAndAtisDeltasApply(t *testing.T) {
	f := NewFeed("http://example.invalid")
	if err := f.applyEvent("snapshot", []byte(realSnapshotEvent)); err != nil {
		t.Fatalf("applyEvent snapshot: %v", err)
	}
	upd := `{"update":4,
		"controllers":{"changed":[{"callsign":"ZSHA_CTR","cid":"1000","latitude":"31.0","longitude":"121.0","visual_range":600}]},
		"atis":{"changed":[{"callsign":"ZSSS_ATIS","cid":"1001","latitude":"31.2","longitude":"121.3","visual_range":0}]}}`
	if err := f.applyEvent("update", []byte(upd)); err != nil {
		t.Fatalf("applyEvent update: %v", err)
	}
	s := f.Snapshot()
	c, ok := s.ByCallsign["ZSHA_CTR"]
	if !ok {
		t.Fatal("the controller delta did not apply")
	}
	if !c.IsATC || c.IsATIS {
		t.Fatalf("ZSHA_CTR = %+v, want IsATC and not IsATIS", c)
	}
	if c.RadiusNM != 600 {
		t.Fatalf("ZSHA_CTR range = %v, want the declared 600", c.RadiusNM)
	}
	a, ok := s.ByCallsign["ZSSS_ATIS"]
	if !ok {
		t.Fatal("the atis delta did not apply")
	}
	if !a.IsATIS {
		t.Fatalf("ZSSS_ATIS = %+v, want IsATIS — the delta group it came from decides this, not the callsign", a)
	}
}

func TestAnUnparsableEventDoesNotWipeTheSnapshot(t *testing.T) {
	f := NewFeed("http://example.invalid")
	if err := f.applyEvent("snapshot", []byte(realSnapshotEvent)); err != nil {
		t.Fatalf("applyEvent: %v", err)
	}
	if err := f.applyEvent("update", []byte("garbage")); err == nil {
		t.Fatal("applyEvent must report a parse failure")
	}
	if _, ok := f.Snapshot().ByCallsign["CCA1"]; !ok {
		t.Fatal("a bad event must leave the previous snapshot intact, not wipe it")
	}
}

func TestAnUnknownEventNameIsIgnoredNotTreatedAsAnError(t *testing.T) {
	// SSE 允许服务端加新的事件类型。把未知事件当成解析失败会让日志被刷屏，
	// 而且那条 warn 说的是"解析不了"，指向完全错误的方向。
	f := NewFeed("http://example.invalid")
	if err := f.applyEvent("snapshot", []byte(realSnapshotEvent)); err != nil {
		t.Fatalf("applyEvent: %v", err)
	}
	if err := f.applyEvent("heartbeat", []byte(`{"whatever":1}`)); err != nil {
		t.Fatalf("an unknown event must be ignored, got %v", err)
	}
	if len(f.Snapshot().ByCallsign) != 2 {
		t.Fatal("an unknown event must not touch the snapshot")
	}
}

// TestTheAtisDoesNotDisplaceTheHumanInTheCidIndex 钉住 cid 冲突的胜负规则。
//
// ATIS 机器人按 {cid}_atis{freq6} 登录，用的就是真实成员的 CID，所以一个人
// 同时以管制员和 ATIS 出现在 datafeed 里是正常运行态——can-fsd 的 golden
// 样本里 CID 1000 正是 ZSHA_CTR 加 ZSSS_ATIS。挑错了不是"有歧义"而已：
// ATIS 是固定机器、visual_range 为 0 走兜底 60 海里，管制员才是要被路由的
// 那个人、600 海里。挑中 ATIS 就是十倍射程误差加错误坐标，且一声不响。
func TestTheAtisDoesNotDisplaceTheHumanInTheCidIndex(t *testing.T) {
	doc := []byte(`{"pilots":[],"controllers":[
		{"callsign":"ZSHA_CTR","cid":"1000","latitude":"31.0","longitude":"121.0","visual_range":600}
	],"atis":[
		{"callsign":"ZSSS_ATIS","cid":"1000","latitude":"31.2","longitude":"121.3","visual_range":0}
	]}`)
	s, err := ParseDatafeed(doc)
	if err != nil {
		t.Fatalf("ParseDatafeed: %v", err)
	}
	p, ok := s.ByCID["1000"]
	if !ok {
		t.Fatal("cid 1000 must resolve to something")
	}
	if p.Callsign != "ZSHA_CTR" {
		t.Fatalf("ByCID[1000] = %s, want the human controller ZSHA_CTR, not the ATIS bot", p.Callsign)
	}
	if p.RadiusNM != 600 {
		t.Fatalf("range = %v, want the controller's 600 (the ATIS would give 60)", p.RadiusNM)
	}
	if _, ok := s.ByCallsign["ZSSS_ATIS"]; !ok {
		t.Fatal("the ATIS must still be reachable by callsign")
	}
}

// TestAnAtisOnlyCidStillResolves 确认上一条规则没有把 ATIS 一刀切掉。
func TestAnAtisOnlyCidStillResolves(t *testing.T) {
	doc := []byte(`{"pilots":[],"controllers":[],"atis":[
		{"callsign":"ZSSS_ATIS","cid":"900","latitude":"31.2","longitude":"121.3","visual_range":0}
	]}`)
	s, err := ParseDatafeed(doc)
	if err != nil {
		t.Fatalf("ParseDatafeed: %v", err)
	}
	if p, ok := s.ByCID["900"]; !ok || p.Callsign != "ZSSS_ATIS" {
		t.Fatalf("ByCID[900] = %+v (ok=%v), want the ATIS", p, ok)
	}
}

// TestTheCidIndexIsDeterministic 钉住第 4 条不变量。
//
// 服务端 ATIS 机队整队共用一个 ATIS_CID（can-audio 里默认 900），所以"同一个
// cid 下挂着多个 ATIS 条目"是必然发生的。Go 的 map 遍历是随机的，can-fsd 那边
// 的数组顺序也是（Clients() 遍历 map），所以靠顺序决定胜负会让同一批人在两个
// tick 里得到不同的路由。
func TestTheCidIndexIsDeterministic(t *testing.T) {
	doc := []byte(`{"pilots":[],"controllers":[],"atis":[
		{"callsign":"ZSSS_ATIS","cid":"900","latitude":"31.2","longitude":"121.3","visual_range":0},
		{"callsign":"ZBAA_ATIS","cid":"900","latitude":"40.0","longitude":"116.6","visual_range":0},
		{"callsign":"ZGGG_ATIS","cid":"900","latitude":"23.4","longitude":"113.3","visual_range":0}
	]}`)
	first, err := ParseDatafeed(doc)
	if err != nil {
		t.Fatalf("ParseDatafeed: %v", err)
	}
	want := first.ByCID["900"].Callsign
	if want == "" {
		t.Fatal("cid 900 must resolve to one of the three")
	}
	for i := 0; i < 50; i++ {
		s, err := ParseDatafeed(doc)
		if err != nil {
			t.Fatalf("ParseDatafeed: %v", err)
		}
		if got := s.ByCID["900"].Callsign; got != want {
			t.Fatalf("run %d picked %s, the first run picked %s — the cid index depends on map iteration order", i, got, want)
		}
	}
}

// TestMergingConvergesOnTheSameSnapshotAsReplacing 钉住第 1 条不变量。
//
// 这是整个任务第二重要的测试：增量路径和全量路径必须给出同一个答案。做不到的
// 话，两者的差异随运行时间累积，而收到下一份全量时又会突然消失。
func TestMergingConvergesOnTheSameSnapshotAsReplacing(t *testing.T) {
	base := []byte(`{"pilots":[
		{"callsign":"CCA1","cid":"1","latitude":30.0,"longitude":120.0,"altitude":10000}
	],"controllers":[
		{"callsign":"ZSHA_CTR","cid":"1000","latitude":"31.0","longitude":"121.0","visual_range":600}
	],"atis":[
		{"callsign":"ZSSS_ATIS","cid":"1000","latitude":"31.2","longitude":"121.3","visual_range":0}
	]}`)

	f := NewFeed("http://example.invalid")
	if err := f.applyEvent("snapshot", base); err != nil {
		t.Fatalf("applyEvent snapshot: %v", err)
	}
	if err := f.applyEvent("update", []byte(`{"update":9,"controllers":{"removed":["ZSHA_CTR"]}}`)); err != nil {
		t.Fatalf("applyEvent update: %v", err)
	}
	merged := f.Snapshot()

	replaced, err := ParseDatafeed([]byte(`{"pilots":[
		{"callsign":"CCA1","cid":"1","latitude":30.0,"longitude":120.0,"altitude":10000}
	],"controllers":[],"atis":[
		{"callsign":"ZSSS_ATIS","cid":"1000","latitude":"31.2","longitude":"121.3","visual_range":0}
	]}`))
	if err != nil {
		t.Fatalf("ParseDatafeed: %v", err)
	}

	if !reflect.DeepEqual(merged.ByCallsign, replaced.ByCallsign) {
		t.Fatalf("ByCallsign diverged:\n merged   = %+v\n replaced = %+v", merged.ByCallsign, replaced.ByCallsign)
	}
	// 这一条才是真正容易错的：管制员走了以后，cid 1000 必须让位给同 cid 的
	// ATIS，而不是变成查不到。增量维护 cid 索引的写法在这里必然出错——
	// 它只会把管制员那条删掉。
	if !reflect.DeepEqual(merged.ByCID, replaced.ByCID) {
		t.Fatalf("ByCID diverged:\n merged   = %+v\n replaced = %+v", merged.ByCID, replaced.ByCID)
	}
	if p, ok := merged.ByCID["1000"]; !ok || p.Callsign != "ZSSS_ATIS" {
		t.Fatalf("after the controller left, cid 1000 = %+v (ok=%v), want the ATIS to take over", p, ok)
	}
}

// TestAnUpdateBeforeTheFirstSnapshotLeavesUsDegraded 钉住第 3 条不变量。
func TestAnUpdateBeforeTheFirstSnapshotLeavesUsDegraded(t *testing.T) {
	f := NewFeed("http://example.invalid")
	if !f.Degraded() {
		t.Fatal("a fresh Feed must start degraded")
	}
	if err := f.applyEvent("update", []byte(realUpdateEvent)); err != nil {
		t.Fatalf("applyEvent: %v", err)
	}
	if !f.Degraded() {
		t.Fatal("an update is not proof that we know the whole network; only a snapshot clears degraded")
	}
	if err := f.applyEvent("snapshot", []byte(realSnapshotEvent)); err != nil {
		t.Fatalf("applyEvent: %v", err)
	}
	if f.Degraded() {
		t.Fatal("a successful snapshot must clear degraded")
	}
}

// TestAReturnedSnapshotIsNeverMutatedInPlace 钉住第 2 条不变量。
//
// Snapshot 的文档契约承诺调用方可以持有一份快照任意长时间而它不会变。
// 合并时就地写进 f.snap 的 map 会违反它，而且那是对调用方正在读的 map
// 的并发写——真正的数据竞争。`go test -race` 只有在真的有人跨越一次
// update 持有旧快照时才看得到，所以这个测试必须显式地那样做。
func TestAReturnedSnapshotIsNeverMutatedInPlace(t *testing.T) {
	f := NewFeed("http://example.invalid")
	if err := f.applyEvent("snapshot", []byte(realSnapshotEvent)); err != nil {
		t.Fatalf("applyEvent: %v", err)
	}

	held := f.Snapshot()
	heldLen := len(held.ByCallsign)
	heldLat := held.ByCallsign["CCA1"].Lat

	upd := []byte(`{"update":5,"pilots":{
		"changed":[
			{"callsign":"CCA1","cid":"1","latitude":99.0,"longitude":120.0,"altitude":10000},
			{"callsign":"CCA3","cid":"3","latitude":32.0,"longitude":122.0,"altitude":30000}
		],
		"removed":["CCA2"]}}`)
	if err := f.applyEvent("update", upd); err != nil {
		t.Fatalf("applyEvent: %v", err)
	}

	if got := len(held.ByCallsign); got != heldLen {
		t.Fatalf("the held snapshot changed size to %d (was %d) — applyEvent mutated a map it had already handed out", got, heldLen)
	}
	if got := held.ByCallsign["CCA1"].Lat; got != heldLat {
		t.Fatalf("the held snapshot's CCA1 moved to %v (was %v) — applyEvent mutated a map it had already handed out", got, heldLat)
	}
	if _, ok := held.ByCallsign["CCA3"]; ok {
		t.Fatal("CCA3 appeared in a snapshot taken before it existed")
	}
}

// TestStreamConsumesARealEventSequence 是 Feed 那一半的第一个真测试。
//
// Task 5 完全没测 Feed，这正是把 update 的线格式认错还能出厂的原因：一个回放
// 真实事件序列的 fixture 会当场红。顺带覆盖真实 SSE 流会做而单个完美 fixture
// 不会做的事：注释行（心跳）、retry 行、以及一个事件带多行 data:。
func TestStreamConsumesARealEventSequence(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		fl, ok := w.(http.Flusher)
		if !ok {
			t.Error("test server response is not flushable")
			return
		}
		io.WriteString(w, ": keepalive\n")
		io.WriteString(w, "retry: 3000\n")
		io.WriteString(w, "event: snapshot\n")
		// 故意拆成多行 data:。SSE 的语义是用换行连接起来；逐行分发会把一份
		// JSON 拆成两半，两半都解析失败，表现为"偶尔丢一个 tick"。
		io.WriteString(w, "data: {\"pilots\":[{\"callsign\":\"CCA1\",\"cid\":\"1\",\n")
		io.WriteString(w, "data: \"latitude\":30.0,\"longitude\":120.0,\"altitude\":10000}],\n")
		io.WriteString(w, "data: \"controllers\":[],\"atis\":[]}\n")
		io.WriteString(w, "\n")
		fl.Flush()
		io.WriteString(w, "event: update\n")
		io.WriteString(w, "data: {\"update\":1,\"pilots\":{\"changed\":[{\"callsign\":\"CCA9\",\"cid\":\"9\",\"latitude\":35.0,\"longitude\":125.0,\"altitude\":20000}]}}\n")
		io.WriteString(w, "\n")
		fl.Flush()
		<-r.Context().Done()
	}))
	defer srv.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	f := NewFeed(srv.URL)
	done := make(chan struct{})
	go func() { defer close(done); f.Run(ctx) }()

	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		s := f.Snapshot()
		_, gotSnap := s.ByCallsign["CCA1"]
		_, gotUpd := s.ByCallsign["CCA9"]
		if gotSnap && gotUpd && !f.Degraded() {
			cancel()
			<-done
			return
		}
		time.Sleep(5 * time.Millisecond)
	}
	s := f.Snapshot()
	cancel()
	<-done
	t.Fatalf("the event sequence never fully applied: ByCallsign = %v, degraded = %v", s.ByCallsign, f.Degraded())
}

// TestASilentStreamEventuallyGoesDegraded 钉住第 5 条不变量。
//
// 连接建立了、头发完了、然后一个字节都不再来——这就是 TCP 流被黑洞化的样子
// （NAT 空闲丢表、防火墙不回 RST）。没有看门狗的话 sc.Scan() 永久阻塞，
// stream() 不返回，Run 里那句 degraded = true 永远执行不到，服务器拿着几小时
// 前的位置继续满怀信心地过滤。
//
// 这个测试把 feedIdleTimeout 临时调小，否则要跑 30 秒。
func TestASilentStreamEventuallyGoesDegraded(t *testing.T) {
	orig := feedIdleTimeout
	feedIdleTimeout = 150 * time.Millisecond
	defer func() { feedIdleTimeout = orig }()

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		fl := w.(http.Flusher)
		fmt.Fprintf(w, "event: snapshot\ndata: %s\n\n", realSnapshotEvent)
		fl.Flush()
		<-r.Context().Done() // 此后永远沉默
	}))
	defer srv.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	f := NewFeed(srv.URL)
	done := make(chan struct{})
	go func() { defer close(done); f.Run(ctx) }()

	// 先等它连上并清掉 degraded。窗口要明显大于 reconnectDelay（5 秒）：
	// 太窄的话偶尔会错过"已经上线"的那一刻，Run 转去睡满 reconnectDelay，
	// 撞穿测试自己的 deadline，然后以一条指向错误原因的信息失败——
	// 这个测试要盯的是看门狗，不是连接建立的速度。
	deadline := time.Now().Add(6 * time.Second)
	for time.Now().Before(deadline) && f.Degraded() {
		time.Sleep(5 * time.Millisecond)
	}
	if f.Degraded() {
		cancel()
		<-done
		t.Fatal("the feed never came up at all")
	}

	// 然后等看门狗发现流已经死了。
	deadline = time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		if f.Degraded() {
			cancel()
			<-done
			return
		}
		time.Sleep(5 * time.Millisecond)
	}
	cancel()
	<-done
	t.Fatal("a stream that went silent never made the feed degraded; the watchdog is missing or not firing")
}

// TestConcurrentReadersAgainstALiveStream 是 Task 5 缺掉的那个测试。
//
// Task 5 的测试全是单 goroutine 的，所以那次干净的 `-race` 什么也没证明——
// 竞态检测器只报告真的并发发生过的访问。这里让 Run 对着一个不停推送的假
// SSE 服务端跑，同时若干 goroutine 猛打 Snapshot/Degraded，并且**遍历返回
// 的 map 内容**（只拿到 Snapshot 结构体而不读 map 是测不出就地修改的）。
//
// 必须用 `go test -race` 跑才有意义。
func TestConcurrentReadersAgainstALiveStream(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		fl, ok := w.(http.Flusher)
		if !ok {
			t.Error("test server response is not flushable")
			return
		}
		fmt.Fprintf(w, "event: snapshot\ndata: %s\n\n", realSnapshotEvent)
		fl.Flush()
		for i := 0; ; i++ {
			if r.Context().Err() != nil {
				return
			}
			fmt.Fprintf(w, "event: update\ndata: {\"update\":%d,\"pilots\":{\"changed\":[{\"callsign\":\"CCA%d\",\"cid\":\"%d\",\"latitude\":30.0,\"longitude\":120.0,\"altitude\":10000}],\"removed\":[\"CCA%d\"]}}\n\n",
				i, i%8, i%8, (i+4)%8)
			fl.Flush()
			time.Sleep(time.Millisecond)
		}
	}))
	defer srv.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	f := NewFeed(srv.URL)
	var wg sync.WaitGroup
	wg.Add(1)
	go func() { defer wg.Done(); f.Run(ctx) }()

	// 读者必须真的读到过东西。没有这个断言，这个测试对着一个死端点也会绿——
	// 而它存在的全部意义就是让 -race 有东西可看：竞态检测器只报告真的并发
	// 发生过的访问。Task 5 那次"干净的 -race"正是这么来的。
	var seen atomic.Int64
	var everUp atomic.Bool

	for i := 0; i < 8; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			deadline := time.Now().Add(300 * time.Millisecond)
			for time.Now().Before(deadline) {
				s := f.Snapshot()
				// 真的读 map 的内容——只取结构体是测不出就地修改的。
				for cs, p := range s.ByCallsign {
					_, _ = cs, p.Lat
					seen.Add(1)
				}
				for cid := range s.ByCID {
					_ = cid
				}
				if !f.Degraded() {
					everUp.Store(true)
				}
			}
		}()
	}

	time.Sleep(300 * time.Millisecond)
	cancel()
	wg.Wait()

	if seen.Load() == 0 {
		t.Fatal("no reader ever observed a single entry; the stream never delivered anything and this test proved nothing about concurrency")
	}
	if !everUp.Load() {
		t.Fatal("the feed was never observed as not-degraded; the stream never actually came up")
	}
}

// TestAParticipantWithNoPositionYetIsNotPlacedAtNullIsland 是本任务的核心测试。
//
// can-fsd 在第一个位置包到达之前省略 latitude/longitude/altitude。把缺失当成 0
// 会把这个人放在几内亚湾 (0,0)，离任何真实飞机五千海里——他听不到任何人，也没有
// 人听得到他，日志里一条错误都没有。
func TestAParticipantWithNoPositionYetIsNotPlacedAtNullIsland(t *testing.T) {
	doc := []byte(`{"pilots":[
		{"callsign":"CCA1","cid":"1"}
	],"controllers":[
		{"callsign":"ZSHA_CTR","cid":"1000","visual_range":600}
	],"atis":[]}`)
	s, err := ParseDatafeed(doc)
	if err != nil {
		t.Fatalf("ParseDatafeed: %v", err)
	}
	for _, cs := range []string{"CCA1", "ZSHA_CTR"} {
		p, ok := s.ByCallsign[cs]
		if !ok {
			t.Fatalf("%s is missing from the snapshot entirely", cs)
		}
		if p.Known {
			t.Fatalf("%s has no coordinates in the document but Known = true; 0,0 is the Gulf of Guinea, not a position", cs)
		}
	}
}

// TestADeltaEntryWithNoCoordinatesIsAlsoUnknown 覆盖增量那条路径。
//
// 坐标缺失不只出现在全量快照里：can-fsd 自己的 TestFeedDeltaMarshalsOnlyChangedGroups
// 构造的就是 Pilot{Callsign: "CCA101"} 其余全零，而 Latitude 是 *float64 + omitempty,
// 所以一个 changed 条目完全可以没有坐标键。只在 ParseDatafeed 里判 Known、忘了增量
// 路径的话，一架刚连上的飞机会在第一个 update 里被放回几内亚湾。
func TestADeltaEntryWithNoCoordinatesIsAlsoUnknown(t *testing.T) {
	f := NewFeed("http://example.invalid")
	if err := f.applyEvent("snapshot", []byte(realSnapshotEvent)); err != nil {
		t.Fatalf("applyEvent snapshot: %v", err)
	}
	upd := []byte(`{"update":7,"pilots":{"changed":[{"callsign":"CCA101","cid":"101"}]}}`)
	if err := f.applyEvent("update", upd); err != nil {
		t.Fatalf("applyEvent update: %v", err)
	}
	p, ok := f.Snapshot().ByCallsign["CCA101"]
	if !ok {
		t.Fatal("the delta entry did not apply at all")
	}
	if p.Known {
		t.Fatal("a delta entry with no coordinate keys must be unknown, not 0,0")
	}
}

// TestADeltaCanTakeAPositionAway 的方向和上一条相反：一个原本有位置的人，
// 新的 changed 条目不带坐标了。合并是整条替换，所以 Known 必须跟着变回 false。
func TestADeltaCanTakeAPositionAway(t *testing.T) {
	f := NewFeed("http://example.invalid")
	if err := f.applyEvent("snapshot", []byte(realSnapshotEvent)); err != nil {
		t.Fatalf("applyEvent snapshot: %v", err)
	}
	if !f.Snapshot().ByCallsign["CCA1"].Known {
		t.Fatal("CCA1 starts with a known position")
	}
	if err := f.applyEvent("update", []byte(`{"update":8,"pilots":{"changed":[{"callsign":"CCA1","cid":"1"}]}}`)); err != nil {
		t.Fatalf("applyEvent update: %v", err)
	}
	if f.Snapshot().ByCallsign["CCA1"].Known {
		t.Fatal("the new entry carries no coordinates, so the position is no longer known")
	}
}

func TestAnExplicitNullCoordinateIsAlsoUnknown(t *testing.T) {
	doc := []byte(`{"pilots":[{"callsign":"CCA1","cid":"1","latitude":null,"longitude":null,"altitude":null}],"controllers":[],"atis":[]}`)
	s, err := ParseDatafeed(doc)
	if err != nil {
		t.Fatalf("ParseDatafeed: %v", err)
	}
	if s.ByCallsign["CCA1"].Known {
		t.Fatal("an explicit null coordinate must be unknown, not 0,0")
	}
}

// TestAnEmptyStringCoordinateIsUnknown 是把 flexFloat 做成结构体而不是指针的理由。
//
// 指针版会让 "" 走进 UnmarshalJSON、被映射成 0、然后因为指针非 nil 而报告"已知"
// ——同一个"缺失当成零"的错误只是下沉了一层。
func TestAnEmptyStringCoordinateIsUnknown(t *testing.T) {
	doc := []byte(`{"pilots":[],"controllers":[{"callsign":"ZSHA_CTR","cid":"1000","latitude":"","longitude":"","visual_range":600}],"atis":[]}`)
	s, err := ParseDatafeed(doc)
	if err != nil {
		t.Fatalf("ParseDatafeed: %v", err)
	}
	if s.ByCallsign["ZSHA_CTR"].Known {
		t.Fatal("an empty-string coordinate must be unknown, not 0,0")
	}
}

// TestANonNumericCoordinateIsStillAnError 确认上一条没有把真正的坏数据也放过去。
func TestANonNumericCoordinateIsStillAnError(t *testing.T) {
	doc := []byte(`{"pilots":[],"controllers":[{"callsign":"ZSHA_CTR","cid":"1000","latitude":"abc","longitude":"121.0"}],"atis":[]}`)
	if _, err := ParseDatafeed(doc); err == nil {
		t.Fatal("a non-numeric coordinate must be reported, not silently treated as unknown")
	}
}

func TestARealPositionIsKnownIncludingZeroZero(t *testing.T) {
	// 真的有人在 (0,0) 的话，那是一个已知位置。区别在于键在不在，不在于值。
	doc := []byte(`{"pilots":[{"callsign":"CCA1","cid":"1","latitude":0,"longitude":0,"altitude":0}],"controllers":[],"atis":[]}`)
	s, err := ParseDatafeed(doc)
	if err != nil {
		t.Fatalf("ParseDatafeed: %v", err)
	}
	p := s.ByCallsign["CCA1"]
	if !p.Known {
		t.Fatal("coordinates that are present and zero are a known position; presence is what decides, not the value")
	}
	if p.Lat != 0 || p.Lon != 0 {
		t.Fatalf("position = %v,%v, want 0,0", p.Lat, p.Lon)
	}
}

// TestEffectiveRangeCombinesTheTwoKindsCorrectly 是 M4 的核心。
//
// 飞行员的 LOSTermNM 是视距公式的半项，管制席位的 RadiusNM 是权威半径。
// 没有任何一条规则同时对这两种组合，所以合并逻辑必须只存在于一个地方。
func TestEffectiveRangeCombinesTheTwoKindsCorrectly(t *testing.T) {
	fl350 := Position{Callsign: "CCA1", Known: true, LOSTermNM: 230}
	fl350b := Position{Callsign: "CCA2", Known: true, LOSTermNM: 230}
	ctr := Position{Callsign: "ZSHA_CTR", Known: true, IsATC: true, RadiusNM: 600}
	twr := Position{Callsign: "ZSPD_TWR", Known: true, IsATC: true, RadiusNM: 30}

	// 两架飞机：两个半项相加。取 max 会得到 230，凭空把射程砍半。
	if got, ok := EffectiveRangeNM(fl350, fl350b); !ok || got != 460 {
		t.Fatalf("pilot+pilot = %v (ok=%v), want 460 — the two half-terms must be summed", got, ok)
	}
	// 飞行员 + 管制：管制的权威半径说了算。相加会得到 830，让管制员听到射程外。
	if got, ok := EffectiveRangeNM(fl350, ctr); !ok || got != 600 {
		t.Fatalf("pilot+CTR = %v (ok=%v), want the controller's authoritative 600", got, ok)
	}
	if got, ok := EffectiveRangeNM(ctr, fl350); !ok || got != 600 {
		t.Fatalf("CTR+pilot = %v (ok=%v), want 600 — the function must be symmetric", got, ok)
	}
	// 两个席位：取大的。两个都是权威半径，相加没有物理意义。
	if got, ok := EffectiveRangeNM(ctr, twr); !ok || got != 600 {
		t.Fatalf("CTR+TWR = %v (ok=%v), want 600", got, ok)
	}
}

func TestEffectiveRangeRefusesToFilterWhenAPositionIsUnknown(t *testing.T) {
	// 第二个返回值是"要不要过滤"，不是"射程是不是零"。不知道的时候放行——
	// 和 Degraded() 一样的原则。刚连上还没发位置包的飞机正要呼叫放行，
	// 那是最不该把他静音的时刻。
	known := Position{Callsign: "CCA1", Known: true, LOSTermNM: 230}
	unknown := Position{Callsign: "CCA2"}
	if _, ok := EffectiveRangeNM(known, unknown); ok {
		t.Fatal("an unknown position must disable filtering, not produce a range")
	}
	if _, ok := EffectiveRangeNM(unknown, known); ok {
		t.Fatal("must be symmetric")
	}
}

// TestTwoAircraftParkedAtSeaLevelCanHearEachOther 是第三个症状。
//
// 海平面机场的飞机高度 0，半项 1.23√0 = 0，合起来 0 海里，而 Quality(d,0)
// 的回答是"不扇出"。同一个机坪上的两架飞机互相听不见。
func TestTwoAircraftParkedAtSeaLevelCanHearEachOther(t *testing.T) {
	doc := []byte(`{"pilots":[
		{"callsign":"CCA1","cid":"1","latitude":31.143,"longitude":121.805,"altitude":0},
		{"callsign":"CCA2","cid":"2","latitude":31.145,"longitude":121.807,"altitude":0}
	],"controllers":[],"atis":[]}`)
	s, err := ParseDatafeed(doc)
	if err != nil {
		t.Fatalf("ParseDatafeed: %v", err)
	}
	a, b := s.ByCallsign["CCA1"], s.ByCallsign["CCA2"]
	rng, ok := EffectiveRangeNM(a, b)
	if !ok {
		t.Fatal("two aircraft with known positions must be filtered, not passed through")
	}
	if rng < 5 {
		t.Fatalf("two aircraft parked at a sea-level airport get %v NM of range — they are on the same apron and cannot hear each other", rng)
	}
}

func TestAPilotRangeStillGrowsWithAltitude(t *testing.T) {
	// 地面下限不能把高度的影响抹平。
	doc := []byte(`{"pilots":[
		{"callsign":"LOW","cid":"1","latitude":30,"longitude":120,"altitude":0},
		{"callsign":"HIGH","cid":"2","latitude":30,"longitude":120,"altitude":35000}
	],"controllers":[],"atis":[]}`)
	s, err := ParseDatafeed(doc)
	if err != nil {
		t.Fatalf("ParseDatafeed: %v", err)
	}
	low, high := s.ByCallsign["LOW"], s.ByCallsign["HIGH"]
	if !(high.LOSTermNM > 200) {
		t.Fatalf("FL350 gives a %v NM half-term, want about 230", high.LOSTermNM)
	}
	if !(low.LOSTermNM < 10) {
		t.Fatalf("a parked aircraft gives a %v NM half-term, want a small floor", low.LOSTermNM)
	}
}

func TestControllerRadiusStillComesFromVisualRange(t *testing.T) {
	doc := []byte(`{"pilots":[],"controllers":[
		{"callsign":"ZSHA_CTR","cid":"1000","latitude":"31.0","longitude":"121.0","visual_range":600}
	],"atis":[{"callsign":"ZSSS_ATIS","cid":"1001","latitude":"31.2","longitude":"121.3","visual_range":0}]}`)
	s, err := ParseDatafeed(doc)
	if err != nil {
		t.Fatalf("ParseDatafeed: %v", err)
	}
	if got := s.ByCallsign["ZSHA_CTR"].RadiusNM; got != 600 {
		t.Fatalf("controller radius = %v, want the declared 600", got)
	}
	if got := s.ByCallsign["ZSSS_ATIS"].RadiusNM; got != 60 {
		t.Fatalf("atis radius = %v, want the 60 from the suffix table (visual_range was 0)", got)
	}
	// 席位不带 LOS 半项，飞行员不带半径——两种量各归各的字段。
	if s.ByCallsign["ZSHA_CTR"].LOSTermNM != 0 {
		t.Fatal("an ATC position must not carry a line-of-sight term")
	}
}
