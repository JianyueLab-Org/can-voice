// Package fsdfeed 从 can-fsd 的 SSE 流维护一份位置快照，供射程过滤使用。
//
// 这是 can-voice 唯一的出站依赖，而且它的降级是安全的：SSE 断开时
// 退回“不做射程过滤”（等于 Mumble 时代的全球互通），而不是拒绝服务。
// 语音能不能通，比射程真实感重要得多（spec 7.3）。
package fsdfeed

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/geo"
)

// reconnectDelay 是 SSE 连接断开或建立失败后，重试前的等待时间。
const reconnectDelay = 5 * time.Second

// maxEventBytes 是单个 SSE 事件的上限。超过这个尺寸的 datafeed 意味着
// 上游出了问题，而不是网络大了一点。
const maxEventBytes = 8 << 20

// feedIdleTimeout 是流静默多久算作已经死掉。can-fsd 每秒推一次
// （它的 feedPushInterval = 1s），所以 30 秒的静默只可能是连接被黑洞化了
// ——NAT 空闲丢表、防火墙不回 RST。没有这个超时，sc.Scan() 会永久阻塞，
// stream() 永远不返回，于是 Run 里那句 degraded = true 永远执行不到：
// 服务器拿着几小时前的位置继续满怀信心地做射程过滤。"已经不知道了"是
// 唯一必须说出来的状态，而它恰恰会是唯一不会说的那个。
//
// 是变量而不是常量，只为了测试能把它调小——生产代码不要改它。
var feedIdleTimeout = 30 * time.Second

// Position 是一个网络参与者（飞行员或管制/ATIS 席位）的位置与射程。
type Position struct {
	Callsign string
	// CID 是这条记录的成员号。放进 Position 里是为了能从呼号索引反向重建
	// cid 索引——见 indexByCID。
	CID     string
	Lat     float64
	Lon     float64
	AltFt   float64
	RangeNM float64
	IsATC   bool
	// IsATIS 区分 ATIS 席位和真人管制员。两者的 IsATC 都是 true（射程规则
	// 相同），但 cid 冲突时谁该胜出取决于这一位。它由条目来自哪个集合决定，
	// 不看呼号后缀——can-fsd 那边就是按集合分的。
	IsATIS bool
}

// Snapshot 是某一时刻的全网位置，按 cid 和呼号两路索引。
//
// cid 是主键——鉴权 token 里本来就带着它，所以射程过滤正常情况下不用
// 协议里加一个字段；呼号只给观察员模式的 follow 用。
//
// 一份 Snapshot 一旦从 Feed.Snapshot 返回，就不会再被就地修改：
// fsdfeed 产生新数据时永远是整体替换成一份新的 Snapshot（连同它的两个
// map 一起新建），从不修改旧 Snapshot 里的 map。调用方因此可以放心持有
// 一份 Snapshot 任意长时间——它不会在你手上变化——但也永远只是那一刻的
// 快照，想要更新的数据必须重新调用 Snapshot。
type Snapshot struct {
	ByCID      map[string]Position
	ByCallsign map[string]Position
}

// flexFloat 吃 JSON 数字也吃 JSON 字符串。
//
// can-fsd 的 datafeed 里飞行员的经纬度是数字而管制员/ATIS 的是字符串，
// 这是它刻意的、由 can-fsd 自己的 testdata/datafeed_golden.json 钉住的
// 契约（不是这里要绕过的 bug）。只当数字解析会让所有管制员落在 0,0
// （几内亚湾），表现为管制员谁都听不见，而日志里一条错误都没有。
type flexFloat float64

func (f *flexFloat) UnmarshalJSON(b []byte) error {
	s := strings.TrimSpace(string(b))
	if s == "null" || s == `""` {
		*f = 0
		return nil
	}
	s = strings.Trim(s, `"`)
	v, err := strconv.ParseFloat(s, 64)
	if err != nil {
		return fmt.Errorf("coordinate %q is neither a JSON number nor a numeric string: %w", b, err)
	}
	*f = flexFloat(v)
	return nil
}

// datafeed 镜像 can-fsd `/v1/data.json`（以及 SSE `snapshot` 事件）的文档
// 形状，只取射程过滤需要的字段。
type datafeed struct {
	Pilots      []pilotEntry `json:"pilots"`
	Controllers []atcEntry   `json:"controllers"`
	ATIS        []atcEntry   `json:"atis"`
}

// pilotEntry 是 datafeed 里一架飞机的形状，只取射程过滤需要的字段。
type pilotEntry struct {
	Callsign string    `json:"callsign"`
	CID      string    `json:"cid"`
	Lat      flexFloat `json:"latitude"`
	Lon      flexFloat `json:"longitude"`
	Altitude flexFloat `json:"altitude"`
}

// atcEntry 是管制员和 ATIS 共用的形状：两者的射程规则相同
// （visual_range 权威，为 0 时退到后缀兜底表），只有 IsATC 标记不同。
type atcEntry struct {
	Callsign    string    `json:"callsign"`
	CID         string    `json:"cid"`
	Lat         flexFloat `json:"latitude"`
	Lon         flexFloat `json:"longitude"`
	VisualRange int       `json:"visual_range"`
}

// deltaGroup 是 can-fsd feedDelta 里一个集合的增量。
//
// 注意：在 update 事件里，pilots/controllers/atis 是**这个对象**，
// 不是 datafeed 里的那个数组。把两者当成同一个形状的话，每个 update 都会
// 报 "cannot unmarshal object into Go struct field"——于是快照冻结在连接
// 那一刻，而 degraded 停在 false，服务器拿着过期位置满怀信心地过滤。
type deltaGroup[T any] struct {
	Changed []T      `json:"changed"`
	Removed []string `json:"removed"`
}

// feedDelta 镜像 can-fsd `update` 事件的形状
// （can-fsd/internal/api/events.go 的 feedDelta）。
// 每个集合都是 omitempty 的指针：安静的 tick 发出来就是 {"update":N}，
// 那**不是**"全网清空"。
type feedDelta struct {
	Pilots      *deltaGroup[pilotEntry] `json:"pilots"`
	Controllers *deltaGroup[atcEntry]   `json:"controllers"`
	ATIS        *deltaGroup[atcEntry]   `json:"atis"`
}

// pilotPosition 把一条 pilot 记录折算成位置。
func pilotPosition(e pilotEntry) Position {
	// 飞行员的射程由高度算，不用 datafeed 自带的 visual_range：
	// VHF 是视距传播，6897 英尺就是约 102 海里，而样本里这条记录的
	// visual_range 是 40——用它会把射程砍掉 60%。
	return Position{
		Callsign: e.Callsign,
		CID:      e.CID,
		Lat:      float64(e.Lat),
		Lon:      float64(e.Lon),
		AltFt:    float64(e.Altitude),
		RangeNM:  geo.LineOfSightNM(float64(e.Altitude), 0),
	}
}

// atcPosition 把一条管制/ATIS 记录折算成位置。isATIS 由它来自哪个集合决定。
func atcPosition(e atcEntry, isATIS bool) Position {
	// visual_range 是权威值，由管制员/ATIS 在 #AA 里声明。
	// 为 0 时（样本里 ZSSS_ATIS 正是 0）退回席位后缀兜底表。
	r := float64(e.VisualRange)
	if r <= 0 {
		r = geo.FallbackRangeNM(e.Callsign)
	}
	return Position{
		Callsign: e.Callsign,
		CID:      e.CID,
		Lat:      float64(e.Lat),
		Lon:      float64(e.Lon),
		RangeNM:  r,
		IsATC:    true,
		IsATIS:   isATIS,
	}
}

// indexByCID 从呼号索引派生 cid 索引。
//
// cid 索引是派生值而不是独立维护的状态，这一点是有意的：增量维护两份索引
// 意味着两条路径各写一份合并逻辑，而它们迟早会漂。漂的表现是"跑了几天位置
// 就不准了，重连一次又好了"。
//
// 同一个 cid 下挂两个条目是正常运行态：ATIS 机器人按 {cid}_atis{freq6}
// 登录，用的就是真实成员的 CID（can-fsd 的 golden 样本里 CID 1000 正是
// ZSHA_CTR 加 ZSSS_ATIS），而服务端 ATIS 机队整队共用一个 ATIS_CID。
// 胜负规则：
//  1. 非 ATIS 胜过 ATIS。ATIS 是固定机器、兜底射程 60 海里，管制员才是
//     要被路由的那个人、600 海里——挑错了是十倍射程误差加错误坐标。
//  2. 同类之间按呼号字典序取小。纯粹为了确定性：Go 的 map 遍历是随机的，
//     can-fsd 那边的数组顺序也是（Clients() 遍历 map），靠顺序决定会让
//     同一批人在两个 tick 里得到不同的路由。
func indexByCID(byCallsign map[string]Position) map[string]Position {
	out := make(map[string]Position, len(byCallsign))
	for _, p := range byCallsign {
		if p.CID == "" {
			continue
		}
		if prev, ok := out[p.CID]; ok && !betterForCID(p, prev) {
			continue
		}
		out[p.CID] = p
	}
	return out
}

// betterForCID 报告在 cid 冲突里 a 是否该取代 b。
func betterForCID(a, b Position) bool {
	if a.IsATIS != b.IsATIS {
		return !a.IsATIS
	}
	return a.Callsign < b.Callsign
}

// ParseDatafeed 把一份 datafeed 文档（data.json 或者 SSE `snapshot` 事件的
// data）折算成位置快照。
func ParseDatafeed(b []byte) (Snapshot, error) {
	var d datafeed
	if err := json.Unmarshal(b, &d); err != nil {
		// 失败时返回的空快照也要带非 nil 的 map：NewFeed 建立了"两个 map
		// 永不为 nil"的不变量，返回 nil 会让调用方在完全无关的地方 panic。
		return emptySnapshot(), fmt.Errorf("parse datafeed: %w", err)
	}

	byCallsign := make(map[string]Position, len(d.Pilots)+len(d.Controllers)+len(d.ATIS))
	for _, e := range d.Pilots {
		if e.Callsign == "" {
			// 没有呼号的记录无法被任何一路索引引用。can-fsd 不会发出这种
			// 记录（呼号是它的主键），出现了就是文档坏了，跳过好过半收。
			continue
		}
		byCallsign[e.Callsign] = pilotPosition(e)
	}
	for i, group := range [2][]atcEntry{d.Controllers, d.ATIS} {
		isATIS := i == 1
		for _, e := range group {
			if e.Callsign == "" {
				continue
			}
			byCallsign[e.Callsign] = atcPosition(e, isATIS)
		}
	}
	return Snapshot{ByCallsign: byCallsign, ByCID: indexByCID(byCallsign)}, nil
}

func emptySnapshot() Snapshot {
	return Snapshot{ByCID: map[string]Position{}, ByCallsign: map[string]Position{}}
}

// Feed 持续消费 can-fsd 的 SSE 流（GET /v1/events）并维护最新快照。
//
// 并发契约：mu 保护 snap 和 degraded 这两个字段的读写。Run 是唯一的写者
// （单个 goroutine，由调用方启动一次），Snapshot/Degraded 可以被任意数量
// 的 goroutine 并发调用——它们各自只做一次 RLock 读出当前值就返回，
// 不会长期持锁,也不会互相阻塞。
type Feed struct {
	url string

	mu       sync.RWMutex
	snap     Snapshot
	degraded bool
}

// NewFeed 建一个 Feed，尚未连接。初始状态是降级的——在第一份快照到达之前
// 我们对谁在哪里一无所知，此时必须让 fan-out 端全部放行而不是全部屏蔽，
// 所以 Degraded() 从 true 开始，且 snap 是两个空 map 而不是 nil map，
// 调用方不用先判 nil 就能安全地查找。
func NewFeed(url string) *Feed {
	return &Feed{
		url:      url,
		snap:     emptySnapshot(),
		degraded: true,
	}
}

// Snapshot 返回当前的位置快照。
//
// 返回值可以被调用方长期持有：它是某一时刻的完整拷贝语义——Feed 更新时
// 永远是新建一份 Snapshot（连同它的两个 map）整体替换掉 f.snap，从不
// 修改已经发出去的旧 Snapshot 里的 map。所以拿到的这份数据不会在你手里
// 变化，但也永远不会自动变新；要看到更新只能再调用一次 Snapshot。
func (f *Feed) Snapshot() Snapshot {
	f.mu.RLock()
	defer f.mu.RUnlock()
	return f.snap
}

// Degraded 报告位置信息当前是否不可用（尚未连上，或连接已经断开）。
// 为 true 时调用方必须跳过射程过滤而全部扇出——退化成 Mumble 时代的
// 全球互通行为，而不是把所有人都屏蔽掉；宁可放得太宽也不要谁都听不见。
func (f *Feed) Degraded() bool {
	f.mu.RLock()
	defer f.mu.RUnlock()
	return f.degraded
}

// Run 连接 SSE 流并持续更新快照，直到 ctx 被取消。
// 连接断开、建立失败、或者流从未可达，都在这里被自动重连吸收——
// can-fsd 是否在线不应该影响 can-voice 能不能启动和提供服务。
// 应当以自己的 goroutine 调用；同一个 Feed 不要并发调用两次 Run。
func (f *Feed) Run(ctx context.Context) {
	for ctx.Err() == nil {
		if err := f.stream(ctx); err != nil {
			if ctx.Err() == nil {
				slog.Warn("fsd feed dropped, falling back to no range filtering", "error", err)
			}
		} else if ctx.Err() == nil {
			slog.Info("fsd feed closed the stream, reconnecting")
		}
		f.mu.Lock()
		f.degraded = true
		f.mu.Unlock()

		select {
		case <-ctx.Done():
			return
		case <-time.After(reconnectDelay):
		}
	}
}

// applyEvent 把一个 SSE 事件并入快照。
//
// snapshot 携带完整的 datafeed 文档，整体替换；update 携带 feedDelta，
// 只带变化的条目和消失的呼号，必须合并。两者**不是同一个形状**——见
// feedDelta 的注释。
func (f *Feed) applyEvent(event string, data []byte) error {
	switch event {
	case "snapshot":
		s, err := ParseDatafeed(data)
		if err != nil {
			// 解析失败时保留上一份快照：一个坏事件不该让全网瞬间失去射程。
			return err
		}
		f.mu.Lock()
		defer f.mu.Unlock()
		f.snap = s
		f.degraded = false
		return nil

	case "update":
		changed, removed, err := parseDelta(data)
		if err != nil {
			return err
		}
		f.mu.Lock()
		defer f.mu.Unlock()

		// 合并必须产出全新的 map。Snapshot 的文档契约承诺"已经返回出去的
		// 快照不会再被就地修改"，而调用方可能正拿着上一份在读；就地写进
		// f.snap.ByCallsign 就是对那份 map 的并发写。这种竞态只在真有调用方
		// 跨越一次 update 持有旧快照时才触发，单 goroutine 的测试看不到。
		merged := make(map[string]Position, len(f.snap.ByCallsign)+len(changed))
		for cs, p := range f.snap.ByCallsign {
			merged[cs] = p
		}
		for cs, p := range changed {
			merged[cs] = p
		}
		// 最后删：can-fsd 的 diffFeeds 保证一个呼号不会同时出现在 changed
		// 和 removed 里，所以顺序其实无所谓，定死一个免得以后猜。
		for _, cs := range removed {
			delete(merged, cs)
		}
		// cid 索引整个重建而不是增量维护——离线一个管制员之后，同 cid 的
		// ATIS 必须顶上来，而增量维护只会把管制员那条删掉、留下一个查不到
		// 的 cid，于是增量路径和全量路径就此发散。
		f.snap = Snapshot{ByCallsign: merged, ByCID: indexByCID(merged)}

		// degraded 不在这里清除：一个增量证明不了我们掌握了全网，而在第一份
		// 全量到达之前把稀疏的位置表当成权威，等于屏蔽所有不在表里的人——
		// 降级本来就是为了宁可全放行也不要全屏蔽。
		return nil

	default:
		// 未知事件名忽略，不当成错误。SSE 允许服务端加新的事件类型，而把
		// 未知事件报成解析失败会刷屏，且那条 warn 指向完全错误的方向。
		slog.Debug("ignoring an unknown feed event", "event", event)
		return nil
	}
}

// parseDelta 解析一个 update 事件，返回变化的条目（按呼号）和离线的呼号。
func parseDelta(b []byte) (map[string]Position, []string, error) {
	var d feedDelta
	if err := json.Unmarshal(b, &d); err != nil {
		return nil, nil, fmt.Errorf("parse feed delta: %w", err)
	}
	changed := map[string]Position{}
	var removed []string

	if g := d.Pilots; g != nil {
		for _, e := range g.Changed {
			if e.Callsign != "" {
				changed[e.Callsign] = pilotPosition(e)
			}
		}
		removed = append(removed, g.Removed...)
	}
	for i, g := range [2]*deltaGroup[atcEntry]{d.Controllers, d.ATIS} {
		if g == nil {
			continue
		}
		isATIS := i == 1
		for _, e := range g.Changed {
			if e.Callsign != "" {
				changed[e.Callsign] = atcPosition(e, isATIS)
			}
		}
		removed = append(removed, g.Removed...)
	}
	return changed, removed, nil
}

// stream 打开一次 SSE 连接并逐行消费，直到连接断开或 ctx 取消。
func (f *Feed) stream(parent context.Context) error {
	ctx, cancel := context.WithCancel(parent)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, f.url, nil)
	if err != nil {
		return err
	}
	req.Header.Set("Accept", "text/event-stream")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("fsd feed returned %s", resp.Status)
	}

	slog.Info("fsd feed connected", "url", f.url)

	// 看门狗：每读到一行就续期。到期就取消请求，让 sc.Scan() 返回，
	// stream() 得以返回，Run 才有机会把 degraded 置位。
	// 计时器在"刚好有一行到达"的瞬间到期会白白重连一次——代价是一次
	// reconnectDelay，比永远卡死好得多。
	watchdog := time.AfterFunc(feedIdleTimeout, func() {
		slog.Warn("fsd feed went silent, dropping the connection",
			"idle_timeout", feedIdleTimeout)
		cancel()
	})
	defer watchdog.Stop()

	sc := bufio.NewScanner(resp.Body)
	sc.Buffer(make([]byte, 0, 64<<10), maxEventBytes)
	event := "message"
	var data []byte
	for sc.Scan() {
		watchdog.Reset(feedIdleTimeout)
		line := sc.Text()
		switch {
		case line == "":
			// 空行是事件边界，到这里才分发，然后把事件名和缓冲区复位。
			if len(data) > 0 {
				if err := f.applyEvent(event, data); err != nil {
					slog.Warn("skipping an unparsable feed event", "event", event, "error", err)
				}
			}
			event, data = "message", nil
		case strings.HasPrefix(line, ":"):
			// 注释行。SSE 用它做心跳保活，忽略。
		case strings.HasPrefix(line, "event:"):
			event = strings.TrimSpace(line[len("event:"):])
		case strings.HasPrefix(line, "data:"):
			// 一个事件可以带多行 data:，语义是用换行连接起来。逐行分发会把
			// 一份 JSON 拆成两半，两半都解析失败——表现为偶尔丢一个 tick。
			v := strings.TrimPrefix(line[len("data:"):], " ") // 规范只剥一个前导空格
			if len(data) > 0 {
				data = append(data, '\n')
			}
			data = append(data, v...)
		}
	}
	if err := sc.Err(); err != nil {
		if errors.Is(err, bufio.ErrTooLong) {
			// 重连解决不了这个：同一行会再来一次。把它单独说出来，否则它
			// 看起来就是一条普通的掉线，而实际上是无限重连。
			return fmt.Errorf("a feed event exceeded %d bytes; reconnecting will hit the same line again: %w", maxEventBytes, err)
		}
		return err
	}
	return nil
}
