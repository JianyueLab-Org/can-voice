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

// Position 是一个网络参与者（飞行员或管制/ATIS 席位）的位置与射程。
type Position struct {
	Callsign string
	Lat      float64
	Lon      float64
	AltFt    float64
	RangeNM  float64
	IsATC    bool
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

// datafeed 镜像 can-fsd `/v1/data.json`（以及 SSE 每个事件里）的文档形状，
// 只取射程过滤需要的字段。
type datafeed struct {
	Pilots []struct {
		Callsign string    `json:"callsign"`
		CID      string    `json:"cid"`
		Lat      flexFloat `json:"latitude"`
		Lon      flexFloat `json:"longitude"`
		Altitude flexFloat `json:"altitude"`
	} `json:"pilots"`
	Controllers []atcEntry `json:"controllers"`
	ATIS        []atcEntry `json:"atis"`
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

// ParseDatafeed 把一份 datafeed 文档（data.json 或者 SSE 一个事件的 data）
// 折算成位置快照。
func ParseDatafeed(b []byte) (Snapshot, error) {
	var d datafeed
	if err := json.Unmarshal(b, &d); err != nil {
		return Snapshot{}, fmt.Errorf("parse datafeed: %w", err)
	}

	n := len(d.Pilots) + len(d.Controllers) + len(d.ATIS)
	s := Snapshot{
		ByCID:      make(map[string]Position, n),
		ByCallsign: make(map[string]Position, n),
	}
	add := func(p Position, cid string) {
		if p.Callsign != "" {
			s.ByCallsign[p.Callsign] = p
		}
		if cid != "" {
			s.ByCID[cid] = p
		}
	}

	for _, e := range d.Pilots {
		// 飞行员的射程由高度算，不用 datafeed 自带的 visual_range：
		// VHF 是视距传播，6897 英尺就是约 102 海里，而样本里这条记录的
		// visual_range 是 40——用它会把射程砍掉 60%。
		add(Position{
			Callsign: e.Callsign,
			Lat:      float64(e.Lat),
			Lon:      float64(e.Lon),
			AltFt:    float64(e.Altitude),
			RangeNM:  geo.LineOfSightNM(float64(e.Altitude), 0),
		}, e.CID)
	}

	for _, group := range [2][]atcEntry{d.Controllers, d.ATIS} {
		for _, e := range group {
			// visual_range 是权威值，由管制员/ATIS 在 #AA 里声明。
			// 为 0 时（样本里 ZSSS_ATIS 正是 0）退回席位后缀兜底表。
			r := float64(e.VisualRange)
			if r <= 0 {
				r = geo.FallbackRangeNM(e.Callsign)
			}
			add(Position{
				Callsign: e.Callsign,
				Lat:      float64(e.Lat),
				Lon:      float64(e.Lon),
				RangeNM:  r,
				IsATC:    true,
			}, e.CID)
		}
	}

	return s, nil
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
		url: url,
		snap: Snapshot{
			ByCID:      map[string]Position{},
			ByCallsign: map[string]Position{},
		},
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
		if err := f.stream(ctx); err != nil && ctx.Err() == nil {
			slog.Warn("fsd feed dropped, falling back to no range filtering", "error", err)
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

// stream 打开一次 SSE 连接并逐行消费，直到连接断开或 ctx 取消。
func (f *Feed) stream(ctx context.Context) error {
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
	sc := bufio.NewScanner(resp.Body)
	sc.Buffer(make([]byte, 0, 64<<10), 8<<20)
	for sc.Scan() {
		line := sc.Text()
		// 只关心 data 行；事件名（snapshot / update）目前不影响处理，
		// 因为两者携带的是同一个文档形状，这里整体替换快照。
		// 注意：update 事件其实是增量的，只带发生变化的条目和离线的
		// 呼号；这里的整体替换会让未变化的条目从快照里消失。
		// Task 6 会修正这一点——这个任务先让 snapshot 路径通过测试。
		data, ok := strings.CutPrefix(line, "data:")
		if !ok {
			continue
		}
		snap, err := ParseDatafeed([]byte(strings.TrimSpace(data)))
		if err != nil {
			slog.Warn("skipping an unparsable feed event", "error", err)
			continue
		}
		f.mu.Lock()
		f.snap = snap
		f.degraded = false
		f.mu.Unlock()
	}
	return sc.Err()
}
