package router

import (
	"context"
	"log/slog"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/control"
)

// 本文件只钉一件事：Subscribe 那条截断日志写在 r.mu **之外**。
//
// 抓日志的 handler 是 transport/logging_test.go 那一个的同款副本（这个仓库对
// 这类小件一向是各包各一份，而不是共享一个），手法也一样：hook 在**写日志的
// 那个 goroutine 上**回头去拿同一把锁，于是"在锁里写日志"这个实现会当场自锁。

type logRecord struct {
	Level slog.Level
	Msg   string
}

type capturingHandler struct {
	mu   sync.Mutex
	recs []logRecord
	hook func()
}

func (h *capturingHandler) Enabled(context.Context, slog.Level) bool { return true }

func (h *capturingHandler) Handle(_ context.Context, r slog.Record) error {
	if h.hook != nil {
		h.hook()
	}
	h.mu.Lock()
	h.recs = append(h.recs, logRecord{Level: r.Level, Msg: r.Message})
	h.mu.Unlock()
	return nil
}

func (h *capturingHandler) WithAttrs([]slog.Attr) slog.Handler { return h }
func (h *capturingHandler) WithGroup(string) slog.Handler      { return h }

// matching 按子串取记录：slog.SetDefault 是全局的，别的测试留下的东西跟这里无关。
func (h *capturingHandler) matching(substr string) []logRecord {
	h.mu.Lock()
	defer h.mu.Unlock()
	var out []logRecord
	for _, r := range h.recs {
		if strings.Contains(r.Msg, substr) {
			out = append(out, r)
		}
	}
	return out
}

func captureLogs(t *testing.T, hook func()) *capturingHandler {
	t.Helper()
	h := &capturingHandler{hook: hook}
	prev := slog.Default()
	slog.SetDefault(slog.New(h))
	t.Cleanup(func() { slog.SetDefault(prev) })
	return h
}

// TestTheTruncationLogIsNotWrittenWhileTheRouterLockIsHeld 钉住"锁内算账，锁外说话"。
//
// 为什么这不是洁癖：r.mu 是**全服务端一把**写锁，扇出的每一步都要过它
// （Listeners、MayTransmit、Get）。而 slog 的默认 handler 一路写到 stderr，
// 而 stderr 是**会卡**的——docker 的 json-file 驱动、journald 背压、一个写满的
// 磁盘。日志一卡，持锁的 Subscribe 跟着卡，全网的扇出排队等一次写日志。
// 实测过同款放大：一次持锁 280 微秒就能把 50 次 Listeners 从 11 微秒推到 67 毫秒
// （6039 倍），而一次卡住的 stderr 写比 280 微秒长得多。
//
// 今天这条日志靠**结构**在锁外：持锁那一段被拆成了 subscribeLocked，因为
// `defer r.mu.Unlock()` 会让函数体里任何一句日志都落在锁内。但"结构上很难写回去"
// 不是"钉住了"——把这一句挪回 subscribeLocked 里，或者在这里顺手补一对
// Lock/Unlock，没有任何东西会响。第二轮那条自锁探针只覆盖 outbound.mu。
//
// 手法是确定性的，不靠计时：hook 在写日志的那个 goroutine 上调 SessionCount()，
// 它要的正是 Subscribe 可能还持着的那把 r.mu 的读锁。sync.RWMutex 不可重入，
// 持写锁的 goroutine 再去 RLock 就是永久阻塞——下面那个 2 秒的 select 是它的红。
func TestTheTruncationLogIsNotWrittenWhileTheRouterLockIsHeld(t *testing.T) {
	r := New()
	sess := r.Add(SessionOpts{CID: "1000", MaxTX: 8, MaxRX: 32, Send: func([]byte) {}})

	// 声明要多到连**被拒清单本身**都装不下，那条日志才会写：
	// 上界是 declaredSlack × max(MaxTX, MaxRX) = 4 × 32 = 128，超出的部分只回报
	// 前 maxRejected = 256 个，所以 400 个频率会有 16 个无声无息地消失——
	// 而"截断了却不说"正是那条日志存在的理由。
	rx := make([]uint32, 0, 400)
	for i := 0; i < 400; i++ {
		rx = append(rx, uint32(100000+i))
	}

	h := captureLogs(t, func() {
		// 回头拿 r.mu 的读锁。日志如果是在写锁里写的，这一句就是自锁。
		_ = r.SessionCount()
	})

	done := make(chan control.SubAck, 1)
	go func() { done <- r.Subscribe(sess.ID, control.Sub{RX: rx}) }()

	var ack control.SubAck
	select {
	case ack = <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("Subscribe never returned: the truncation line is being logged while r.mu is held, so anything that slows the log backend down (docker json-file, journald backpressure, a full disk) stalls every fan-out on the server — r.mu is one lock for the whole process")
	}

	// 前提有两半，缺一句这个测试就可能在一条根本没写日志的路径上平凡通过。
	if !ack.RejectedTruncated {
		t.Fatalf("premise: this SUB was not truncated at all (rejected=%d) — the log line under test is never reached, so the assertion above is vacuous", len(ack.Rejected))
	}
	if got := h.matching("rejection list did not fit"); len(got) != 1 {
		t.Fatalf("premise: the truncation line was written %d times, want 1 — an implementation that simply does not log would satisfy the deadlock check for free", len(got))
	}
}
