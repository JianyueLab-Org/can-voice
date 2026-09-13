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
	"strings"
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

// ---- Fix round 1（复审 0 Critical / 1 Major / 4 Minor）----

// TestSustainedParseFailuresGoDegradedAndDropTheStream 钉住 Step 0d 的整个机制。
//
// 这是 Task 5 那个故障模式的看门人：收得到字节但一个都用不了，和收不到字节
// 是同一件事，只是更隐蔽——按行计时的看门狗看到行就续期，所以它永远不响。
// 没有这个测试，把计数器、degraded、Error 日志和 return 全部删掉，整个测试
// 套件依然全绿。
func TestSustainedParseFailuresGoDegradedAndDropTheStream(t *testing.T) {
	// 先发一份能用的 snapshot 让它上线，然后持续发解析不了的 update。
	var served atomic.Int64
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		served.Add(1)
		w.Header().Set("Content-Type", "text/event-stream")
		fl := w.(http.Flusher)
		fmt.Fprintf(w, "event: snapshot\ndata: %s\n\n", realSnapshotEvent)
		fl.Flush()
		for r.Context().Err() == nil {
			fmt.Fprint(w, "event: update\ndata: {\"pilots\":[]}\n\n") // 数组，不是 feedDelta 的对象
			fl.Flush()
			time.Sleep(5 * time.Millisecond)
		}
	}))
	defer srv.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	f := NewFeed(srv.URL)
	done := make(chan struct{})
	go func() { defer close(done); f.Run(ctx) }()

	// 先确认它真的上线过——否则下面的 degraded 可能只是"还没连上"。
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) && f.Degraded() {
		time.Sleep(5 * time.Millisecond)
	}
	if f.Degraded() {
		cancel()
		<-done
		t.Fatal("the feed never came up; this test cannot say anything about parse failures")
	}

	// 然后确认持续的解析失败把它打回降级。
	deadline = time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		if f.Degraded() {
			// 而且快照必须还在——坏事件不该让全网瞬间失去射程。
			if _, ok := f.Snapshot().ByCallsign["CCA1"]; !ok {
				cancel()
				<-done
				t.Fatal("the snapshot was wiped; a stream of bad events must degrade us, not blank us")
			}
			cancel()
			<-done
			return
		}
		time.Sleep(5 * time.Millisecond)
	}
	cancel()
	<-done
	t.Fatal("a stream delivering nothing but unparsable updates never made the feed degraded; receiving bytes you cannot use is the same as receiving nothing, only quieter")
}

// TestAGoodEventResetsTheFailureCounter 钉住计数器会归零，但走的是 httptest
// 路径而不是简报字面给的"直接调 applyEvent"版本。
//
// consecutiveFailures 是 stream() 内部的局部变量：它在事件分发点递增/清零，
// 而 applyEvent 本身从来摸不到它。直接调 f.applyEvent(...) 交替好坏事件的话，
// 这个测试测的是"applyEvent 对连续调用没有奇怪的副作用"，和计数器会不会
// 归零毫无关系——不归零的版本一样会通过，因为压根没有计数器可言。所以改成
// 让假服务端交替发送好/坏事件，次数远超过 maxConsecutiveParseFailures，
// 直接盯 Degraded()：计数器不归零的话，5 个坏事件之后 recordFailure 会把
// degraded 置 true——这一步不需要等重连，Run() 在 stream() 一返回、还没
// 睡 reconnectDelay 之前就已经置位了，所以不用碰 reconnectDelay 这个生产
// 常量，也不用数连接次数。
//
// 复审第二轮验证过：拿 served（连接计数）当观察对象的第一版反而要靠
// reconnectDelay 才能看见断线——那是唯一真的卡在这个延迟后面的东西；
// degraded 本身在 stream() 返回的那一刻就变了，不用等。
func TestAGoodEventResetsTheFailureCounter(t *testing.T) {
	// 必须是单行 JSON：realUpdateEvent 里嵌了原始换行，直接塞进 "data: %s"
	// 会把后续物理行发成没有 "data:" 前缀的续行，被规范正确地当成未知内容
	// 丢弃，表现成"好事件"自己先解析失败——那是这段测试代码的 bug，不是
	// 被测代码的。
	const goodUpdate = `{"update":1,"pilots":{"changed":[{"callsign":"CCA1","cid":"1","latitude":30.5,"longitude":120.5,"altitude":11000}]}}`

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		fl := w.(http.Flusher)
		fmt.Fprintf(w, "event: snapshot\ndata: %s\n\n", realSnapshotEvent)
		fl.Flush()
		for r.Context().Err() == nil {
			// 坏、好交替，次数远超 maxConsecutiveParseFailures。计数器不归零
			// 的话，degraded 很快就会变 true。
			fmt.Fprint(w, "event: update\ndata: {\"pilots\":[]}\n\n") // 数组，解析失败
			fl.Flush()
			fmt.Fprintf(w, "event: update\ndata: %s\n\n", goodUpdate) // 好事件
			fl.Flush()
			time.Sleep(2 * time.Millisecond)
		}
	}))
	defer srv.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	f := NewFeed(srv.URL)
	done := make(chan struct{})
	go func() { defer close(done); f.Run(ctx) }()

	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) && f.Degraded() {
		time.Sleep(5 * time.Millisecond)
	}
	if f.Degraded() {
		cancel()
		<-done
		t.Fatal("the feed never came up")
	}

	// 交替事件持续跑，次数远超 maxConsecutiveParseFailures。计数器不归零的
	// 话，degraded 会在这段窗口内变 true——直接盯它，不用等重连。
	deadline = time.Now().Add(500 * time.Millisecond)
	for time.Now().Before(deadline) {
		if f.Degraded() {
			cancel()
			<-done
			t.Fatal("the feed went degraded while good and bad events were alternating; a good event was not resetting the failure counter")
		}
		time.Sleep(2 * time.Millisecond)
	}
	cancel()
	<-done
}

// TestACallsignMovingBetweenCollectionsIsNotLost 钉住"先删后改"（Step 0a）。
//
// diffFeeds 的不相交只在**单个集合内**成立。一个呼号从 pilots 消失、同一 tick
// 在 controllers 出现时，如果先应用 changed 再应用 removed，它会被直接删掉而
// 不是移过去，并且一直缺到下次重连。
func TestACallsignMovingBetweenCollectionsIsNotLost(t *testing.T) {
	f := NewFeed("http://example.invalid")
	if err := f.applyEvent("snapshot", []byte(realSnapshotEvent)); err != nil {
		t.Fatalf("applyEvent snapshot: %v", err)
	}
	upd := []byte(`{"update":1,
		"pilots":{"removed":["CCA1"]},
		"controllers":{"changed":[{"callsign":"CCA1","cid":"1","latitude":"31.0","longitude":"121.0","visual_range":250}]}}`)
	if err := f.applyEvent("update", upd); err != nil {
		t.Fatalf("applyEvent update: %v", err)
	}
	p, ok := f.Snapshot().ByCallsign["CCA1"]
	if !ok {
		t.Fatal("CCA1 left pilots and appeared in controllers in the same tick, and was deleted instead of moved; it stays missing until the next reconnect")
	}
	if !p.IsATC || p.RadiusNM != 250 {
		t.Fatalf("CCA1 = %+v, want the controller entry", p)
	}
}

// TestSustainedOversizedEventsAlsoGoDegraded 钉住"超限丢弃"和"解析失败"共用
// 同一个计数器和同一个后果（Fix 3）。
//
// 测量过：不共用的话，一个不停发短 data: 行、永远不发空行的对端会让进程
// 在 6 秒内打印 1006 条 warn，而 degraded 停在 false、快照冻结在连接那一刻
// ——这是 Task 5 的故障模式换了一张脸，而 ErrTooLong 分支三行之外早就对同
// 一类问题做出了正确的选择。
//
// 对着生产环境真实的 8 MB maxEventBytes 跑，不缩小它：60 KB 一行的
// data:，约 140 行就能撑爆 8 MB，复审量过在回环网络上 25ms 就能触发——
// 比缩小常量更贴近真实生产路径,也不必操心"缩小的旋钮会不会漏进生产
// 代码"。60 KB 仍然远小于 sc.Buffer 的 64<<10 初始容量之上 bufio.Scanner
// 自己愿意扩到的上限,所以不会被 ErrTooLong 抢先截胡。
func TestSustainedOversizedEventsAlsoGoDegraded(t *testing.T) {
	oversizedLine := strings.Repeat("x", 60<<10)

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		fl := w.(http.Flusher)
		fmt.Fprintf(w, "event: snapshot\ndata: %s\n\n", realSnapshotEvent)
		fl.Flush()
		for r.Context().Err() == nil {
			// 永远不发空行：事件从不分发，只在累积路径里被反复判定超限。
			fmt.Fprintf(w, "data: %s\n", oversizedLine)
			fl.Flush()
		}
	}))
	defer srv.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	f := NewFeed(srv.URL)
	done := make(chan struct{})
	go func() { defer close(done); f.Run(ctx) }()

	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) && f.Degraded() {
		time.Sleep(5 * time.Millisecond)
	}
	if f.Degraded() {
		cancel()
		<-done
		t.Fatal("the feed never came up; this test cannot say anything about the oversized-event path")
	}

	deadline = time.Now().Add(3 * time.Second)
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
	t.Fatal("a stream delivering nothing but oversized events never made the feed degraded")
}

// TestNonFiniteCoordinatesAreUnknown 和 TestNonFiniteAltitudeGivesAFiniteLOSTerm
// 钉住 Fix 4：strconv.ParseFloat 干净地接受 "NaN"/"Inf"/"-Inf"/"+Inf"/"Infinity"，
// 而它们会整个绕开距离过滤——NaN 让每一次比较都为假，所以上游的范围检查
// 放它过去，而 Quality(d, NaN) 判定"在射程内"；Inf 的高度给出无限射程，
// 全球满格。这不是假设的输入：can-fsd 的 ParseLatLon 对 NaN 的边界检查
// 全部失败（比较里带 NaN 恒假），formatCoord 会把它原样吐回来。
func TestNonFiniteCoordinatesAreUnknown(t *testing.T) {
	for _, v := range []string{"NaN", "Inf", "-Inf", "+Inf", "Infinity"} {
		doc := []byte(fmt.Sprintf(
			`{"pilots":[],"controllers":[{"callsign":"ZSHA_CTR","cid":"1000","latitude":%q,"longitude":"121.0","visual_range":600}],"atis":[]}`,
			v))
		s, err := ParseDatafeed(doc)
		if err != nil {
			t.Fatalf("ParseDatafeed(latitude=%q): %v", v, err)
		}
		p := s.ByCallsign["ZSHA_CTR"]
		if p.Known {
			t.Fatalf("latitude=%q gave Known=true; NaN/Inf must be treated as unset, not as a valid coordinate", v)
		}
	}
}

func TestNonFiniteAltitudeGivesAFiniteLOSTerm(t *testing.T) {
	for _, v := range []string{"NaN", "Inf", "-Inf"} {
		doc := []byte(fmt.Sprintf(
			`{"pilots":[{"callsign":"CCA1","cid":"1","latitude":30.0,"longitude":120.0,"altitude":%q}],"controllers":[],"atis":[]}`,
			v))
		s, err := ParseDatafeed(doc)
		if err != nil {
			t.Fatalf("ParseDatafeed(altitude=%q): %v", v, err)
		}
		p := s.ByCallsign["CCA1"]
		if math.IsNaN(p.LOSTermNM) || math.IsInf(p.LOSTermNM, 0) {
			t.Fatalf("altitude=%q gave LOSTermNM=%v, want a finite floored value", v, p.LOSTermNM)
		}
		if !p.Known {
			t.Fatalf("altitude=%q made the whole position unknown, but lat/lon were fine — Known must track lat/lon only", v)
		}
	}
}

// TestAltFtIsTheRawAltitudeNotTheFlooredOne 钉住下限只作用于派生的半项。
// AltFt 会被后面的代码当作真实高度读取（比如显示或判断是否在地面），
// 把下限写进它会让一架停在海平面的飞机报告自己在 20 英尺。
func TestAltFtIsTheRawAltitudeNotTheFlooredOne(t *testing.T) {
	doc := []byte(`{"pilots":[{"callsign":"CCA1","cid":"1","latitude":30,"longitude":120,"altitude":0}],"controllers":[],"atis":[]}`)
	s, err := ParseDatafeed(doc)
	if err != nil {
		t.Fatalf("ParseDatafeed: %v", err)
	}
	p := s.ByCallsign["CCA1"]
	if p.AltFt != 0 {
		t.Fatalf("AltFt = %v, want the raw 0 — minAntennaFt must only floor the derived LOS term", p.AltFt)
	}
	if p.LOSTermNM <= 0 {
		t.Fatalf("LOSTermNM = %v, want the floored value", p.LOSTermNM)
	}
}

// ---- Fix round 2（复审第二轮：5 个新变异，加一处 Part 1 的测试改法）----

// TestWatchdogIsResetByEachLineNotJustAtConnect 钉住"每读到一行就续期"这半句
// （Mutant A）。
//
// TestASilentStreamEventuallyGoesDegraded 已经钉住了"完全沉默会触发看门狗"，
// 但那对着"看门狗只在 stream() 开始时设一次、之后再也不 Reset"的阉割版本
// 同样是绿的——沉默确实会触发，只是原因换了。持续有数据到达但从不续期，
// 同样会在 feedIdleTimeout 之后掉线，这正是 watchdog.Reset 存在的全部
// 理由，却没有任何测试盯着它。
func TestWatchdogIsResetByEachLineNotJustAtConnect(t *testing.T) {
	orig := feedIdleTimeout
	feedIdleTimeout = 100 * time.Millisecond
	defer func() { feedIdleTimeout = orig }()

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		fl := w.(http.Flusher)
		fmt.Fprintf(w, "event: snapshot\ndata: %s\n\n", realSnapshotEvent)
		fl.Flush()
		for r.Context().Err() == nil {
			// 注释行也会喂看门狗——不必是合法事件，只要是一行。
			io.WriteString(w, ": keepalive\n")
			fl.Flush()
			time.Sleep(10 * time.Millisecond)
		}
	}))
	defer srv.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	f := NewFeed(srv.URL)
	done := make(chan struct{})
	go func() { defer close(done); f.Run(ctx) }()

	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) && f.Degraded() {
		time.Sleep(5 * time.Millisecond)
	}
	if f.Degraded() {
		cancel()
		<-done
		t.Fatal("the feed never came up")
	}

	// feedIdleTimeout 调到 100ms，喂养间隔是 10ms——如果 watchdog.Reset 没有
	// 被每一行调用，第一次设的计时器会在 100ms 后无视持续的流量照样触发。
	// 500ms 的窗口对这个差异绰绰有余。
	deadline = time.Now().Add(500 * time.Millisecond)
	for time.Now().Before(deadline) {
		if f.Degraded() {
			cancel()
			<-done
			t.Fatal("the feed went degraded while data kept arriving well inside feedIdleTimeout; the watchdog is not being reset per line")
		}
		time.Sleep(5 * time.Millisecond)
	}
	cancel()
	<-done
}

// TestAHalfCoordinateEntryIsUnknown 钉住 Known 用的是 && 而不是 ||（Mutant C）。
//
// 只有纬度或只有经度，和完全没有坐标是同一类错误：缺的那一半落到零值，
// 也就是错误的经度或纬度——这正是本任务想避免的"当成 0"失败，只是发生在
// 单个轴上而不是整个坐标。同时覆盖飞行员（pilotPosition）和管制/ATIS
// （atcPosition）两条路径，复审指的两处都在这一个测试里。
func TestAHalfCoordinateEntryIsUnknown(t *testing.T) {
	doc := []byte(`{"pilots":[
		{"callsign":"CCA1","cid":"1","latitude":31.0}
	],"controllers":[
		{"callsign":"ZSHA_CTR","cid":"1000","longitude":"121.0","visual_range":600}
	],"atis":[]}`)
	s, err := ParseDatafeed(doc)
	if err != nil {
		t.Fatalf("ParseDatafeed: %v", err)
	}
	if s.ByCallsign["CCA1"].Known {
		t.Fatal("pilot: latitude present but longitude entirely missing; Known must require BOTH, not either")
	}
	if s.ByCallsign["ZSHA_CTR"].Known {
		t.Fatal("controller: longitude present but latitude entirely missing; Known must require BOTH, not either")
	}
}

// TestALargeButValidSnapshotLineIsNotTruncated 钉住 sc.Buffer(...) 那一行
// （Mutant D）。
//
// bufio.Scanner 不设置这个的话，默认单行上限是 64KB（bufio.MaxScanTokenSize）；
// can-fsd 一个繁忙 FIR 的全量快照很容易超过这个数字，同时远小于
// maxEventBytes 的 8MB。删掉这一行之后，第一份快照本身就会在每次连接时
// 触发 bufio.ErrTooLong，永远连不上——而症状只是"一直连不上"，不会指向
// "缓冲区"这个真正原因。
func TestALargeButValidSnapshotLineIsNotTruncated(t *testing.T) {
	// 造一份 100KB 出头、结构仍然合法的快照：填充进一个未知字段不影响解析
	// （json.Unmarshal 默认忽略不认识的键），只影响它的字节数。
	pad := strings.Repeat("A", 100<<10)
	big := fmt.Sprintf(
		`{"pilots":[{"callsign":"CCA1","cid":"1","latitude":30.0,"longitude":120.0,"altitude":10000,"padding":%q}],"controllers":[],"atis":[]}`,
		pad)

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		fl := w.(http.Flusher)
		fmt.Fprintf(w, "event: snapshot\ndata: %s\n\n", big)
		fl.Flush()
		<-r.Context().Done()
	}))
	defer srv.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	f := NewFeed(srv.URL)
	done := make(chan struct{})
	go func() { defer close(done); f.Run(ctx) }()

	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) && f.Degraded() {
		time.Sleep(5 * time.Millisecond)
	}
	if f.Degraded() {
		cancel()
		<-done
		t.Fatal("a snapshot line just over 64KB (bufio's default max) but far under the 8MB limit never came up — the scanner's buffer cap is missing or too small")
	}
	cancel()
	<-done
}

// TestKeepAlivesDoNotResetTheFailureCounter 钉住"空行只有真带了事件才该
// 分发"（Mutant F）。
//
// can-fsd 每 15 秒发一次 ": keepalive\n\n"——一条注释行，跟着一个空行
// （已核对 can-fsd/internal/api/events.go 的 fmt.Fprint(w, ": keepalive\n\n")）。
// 空行是事件边界，只有 len(data) > 0 时才该分发；把这个判断错改成"总是
// 分发"，每个 keepalive 自己的空行都会走进 applyEvent("message", nil)，
// 命中 default 分支返回 nil（"忽略未知事件"），而这个 nil 被 stream() 当成
// "这一轮成功"去清零 consecutiveFailures——于是一串解不开的 update 只要
// 中间夹着 keepalive 就永远凑不满 5 次连续失败。这是 Task 5 的故障模式从
// 侧门走了回来，所以给它复审要求的"最锋利"的测试：交替发送解不开的
// update 和 keepalive，断言最终仍然降级。
func TestKeepAlivesDoNotResetTheFailureCounter(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		fl := w.(http.Flusher)
		fmt.Fprintf(w, "event: snapshot\ndata: %s\n\n", realSnapshotEvent)
		fl.Flush()
		for r.Context().Err() == nil {
			io.WriteString(w, ": keepalive\n\n")
			fl.Flush()
			fmt.Fprint(w, "event: update\ndata: {\"pilots\":[]}\n\n") // 数组，解析失败
			fl.Flush()
			time.Sleep(2 * time.Millisecond)
		}
	}))
	defer srv.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	f := NewFeed(srv.URL)
	done := make(chan struct{})
	go func() { defer close(done); f.Run(ctx) }()

	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) && f.Degraded() {
		time.Sleep(5 * time.Millisecond)
	}
	if f.Degraded() {
		cancel()
		<-done
		t.Fatal("the feed never came up")
	}

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
	t.Fatal("a stream of unparsable updates interleaved with keepalives never made the feed degraded; a keepalive's blank line must not reset the failure counter")
}

// TestABadStatusFailsFastNotViaTheWatchdog 钉住"状态码检查必须在开始读 body
// 之前生效"（Mutant G）。
//
// 一个 404（写错的 feed 地址）或 401（吊销的凭证）应该在连接建立的那一刻
// 就失败——can-fsd 永远不会用非 200 状态码开始推流。把状态检查放宽到
// ">= 500" 的话，4xx 会被当成"打开了一条正常的流"继续往下读；如果那具
// body 什么都不发，失败就只能靠 feedIdleTimeout 兜底——一个写错的 URL
// 或者吊销的凭证，报错会晚上 30 秒，而且报的是"流静默"而不是"状态码不对"。
//
// 用服务端自己观察到"客户端断开连接"花了多久来当信号：正确实现应该在
// 状态检查那一行就 return，defer resp.Body.Close() 几乎立刻断开连接；
// 阉割版本要撑到 feedIdleTimeout（这里调小到 2 秒，仍然明显大于状态检查
// 该花的时间）才会断。
func TestABadStatusFailsFastNotViaTheWatchdog(t *testing.T) {
	orig := feedIdleTimeout
	feedIdleTimeout = 2 * time.Second
	defer func() { feedIdleTimeout = orig }()

	serverSawDone := make(chan time.Duration, 1)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		start := time.Now()
		w.WriteHeader(http.StatusNotFound)
		// 必须显式 Flush：不这样做，状态行留在服务端的缓冲区里，客户端的
		// http.DefaultClient.Do(req) 根本不会返回——连状态码检查都走不到，
		// 这不是在测状态码分支，是在测连接建立本身。
		w.(http.Flusher).Flush()
		// 故意什么都不发——如果客户端把这当成一条正常的流，唯一能让它
		// 结束的就是看门狗。
		<-r.Context().Done()
		select {
		case serverSawDone <- time.Since(start):
		default:
		}
	}))
	defer srv.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	f := NewFeed(srv.URL)
	done := make(chan struct{})
	go func() { defer close(done); f.Run(ctx) }()

	select {
	case elapsed := <-serverSawDone:
		if elapsed > 500*time.Millisecond {
			t.Fatalf("the client gave up after %v; a 404 must fail immediately on the status check, not wait for the %v idle watchdog", elapsed, feedIdleTimeout)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("the server never saw the client disconnect within 3s — the status check for a 404 is not failing fast")
	}
	cancel()
	<-done
}

// TestAGracefulCloseAlsoGoesDegraded 钉住第三条掉进"快照过期却没人知道"的门路。
//
// 前两条是 Task 5 原本的死连接（看门狗管）和 Fix round 1 的超限累积（
// recordFailure 管）；这是第三条：stream() 干干净净地返回 nil——
// sc.Scan() 因为对端正常关闭而返回 false，sc.Err() 是 nil，没有任何错误，
// 也没有看门狗触发。can-fsd 的一次平滑重启，或者中间代理礼貌地断开流，
// 都长这样。Run() 现在把 f.degraded = true 放在 if/else if 之外，对
// 错误返回和干净返回一视同仁地生效；但这行为从来没有被钉住过——如果有人
// 把它挪进 err != nil 分支里（读起来像是在"只在真出错时才降级"，很自然的
// 一次"整理"），干净关闭就会让 degraded 停在 false，快照在整个重连
// 窗口期间继续被当作权威数据，没有任何信号能告诉调用方这份快照已经过期。
func TestAGracefulCloseAlsoGoesDegraded(t *testing.T) {
	closeNow := make(chan struct{})
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		fl := w.(http.Flusher)
		fmt.Fprintf(w, "event: snapshot\ndata: %s\n\n", realSnapshotEvent)
		fl.Flush()
		// 撑住这个响应，等测试确认 degraded 已经因为这份快照变成 false，
		// 再主动、干净地返回——不是被 ctx 取消，也没有出错，就是单纯的
		// "这次响应结束了"，模拟 can-fsd 的一次平滑重启或者代理的礼貌断开。
		select {
		case <-closeNow:
		case <-r.Context().Done():
		}
	}))
	defer srv.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	f := NewFeed(srv.URL)
	done := make(chan struct{})
	go func() {
		defer close(done)
		f.Run(ctx)
	}()

	// 先等它真的连上、吃到快照、退出初始的 degraded=true——这段时间里
	// 服务端一直卡在 select 里没有返回，所以这个窗口是稳定的，不会被
	// "服务端立刻又关闭"抢先盖掉。
	upDeadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(upDeadline) && f.Degraded() {
		time.Sleep(5 * time.Millisecond)
	}
	if f.Degraded() {
		cancel()
		<-done
		t.Fatal("the feed never came up long enough to observe a graceful close")
	}

	// 现在让服务端干净地返回：没有错误，没有被看门狗或 ctx 取消打断。
	close(closeNow)

	// degraded 必须重新变成 true——不需要等下一次重连尝试，Run() 在
	// stream() 一返回就该置位，不管返回值是不是 nil。
	degradedDeadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(degradedDeadline) {
		if f.Degraded() {
			cancel()
			<-done
			return
		}
		time.Sleep(5 * time.Millisecond)
	}
	cancel()
	<-done
	t.Fatal("a graceful close (clean EOF, stream() returned nil) never made the feed degraded")
}

// TestTheRequestCarriesTheAcceptHeader 钉住 req.Header.Set("Accept", ...)
// 这一行。
//
// 现有的假服务端全都不检查请求头，所以删掉这一行不会让本文件里任何一个
// 既有测试变红。真实环境里更严格的 can-fsd 部署，或者中间的反向代理，
// 可能会因为缺这个头而把响应整体缓冲起来再发（SSE 就变成了假的），或者
// 干脆答 406——两种情况看起来都会像"连不上/连上了但一直没数据"，而不是
// "客户端忘了声明自己要 SSE"。
func TestTheRequestCarriesTheAcceptHeader(t *testing.T) {
	headerOK := make(chan bool, 1)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		select {
		case headerOK <- r.Header.Get("Accept") == "text/event-stream":
		default:
		}
		w.Header().Set("Content-Type", "text/event-stream")
		fl := w.(http.Flusher)
		fmt.Fprintf(w, "event: snapshot\ndata: %s\n\n", realSnapshotEvent)
		fl.Flush()
		<-r.Context().Done()
	}))
	defer srv.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	f := NewFeed(srv.URL)
	done := make(chan struct{})
	go func() {
		defer close(done)
		f.Run(ctx)
	}()

	select {
	case ok := <-headerOK:
		cancel()
		<-done
		if !ok {
			t.Fatal(`the request did not carry "Accept: text/event-stream"`)
		}
	case <-time.After(2 * time.Second):
		cancel()
		<-done
		t.Fatal("the server never received a request")
	}
}
