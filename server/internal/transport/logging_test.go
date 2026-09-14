package transport

import (
	"context"
	"errors"
	"log/slog"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/quic-go/quic-go"
)

// 本文件钉住两件"日志本身就是功能"的事：发送队列溢出时那条日志不能在锁里写，
// 以及一次**永久的**发送失败必须出声。
//
// 两条都只能通过日志观察，所以这里有一个抓日志的 handler。它同时是 hook 的
// 挂载点——下面那条死锁探针就是靠在 Handle 里回头去拿同一把锁实现的。

// logRecord 是抓下来的一条日志。
type logRecord struct {
	Level slog.Level
	Msg   string
	Attrs map[string]any
}

// capturingHandler 把日志记进内存，并在**写日志的那个 goroutine 上**同步调用 hook。
type capturingHandler struct {
	mu   sync.Mutex
	recs []logRecord

	// hook 由测试提供，在每条记录被处理时同步调用。
	// 它是"这条日志是在哪个锁下面写的"这个问题唯一的观察手段。
	hook func()
}

func (h *capturingHandler) Enabled(context.Context, slog.Level) bool { return true }

func (h *capturingHandler) Handle(_ context.Context, r slog.Record) error {
	if h.hook != nil {
		h.hook()
	}
	rec := logRecord{Level: r.Level, Msg: r.Message, Attrs: map[string]any{}}
	r.Attrs(func(a slog.Attr) bool {
		rec.Attrs[a.Key] = a.Value.Any()
		return true
	})
	h.mu.Lock()
	h.recs = append(h.recs, rec)
	h.mu.Unlock()
	return nil
}

func (h *capturingHandler) WithAttrs([]slog.Attr) slog.Handler { return h }
func (h *capturingHandler) WithGroup(string) slog.Handler      { return h }

// matching 返回消息里含 substr 的那些记录。
//
// 按子串而不是按全部记录：slog.SetDefault 是全局的，别的测试留下的 goroutine
// 也会往这个 handler 里写，而它们跟这里要断言的事无关。
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

// captureLogs 把默认 logger 换成一个抓取器，测试结束时还原。
func captureLogs(t *testing.T, hook func()) *capturingHandler {
	t.Helper()
	h := &capturingHandler{hook: hook}
	prev := slog.Default()
	slog.SetDefault(slog.New(h))
	t.Cleanup(func() { slog.SetDefault(prev) })
	return h
}

// TestTheOverflowLogIsNotWrittenWhileTheQueueLockIsHeld 钉住"锁里数数，锁外说话"。
//
// 为什么这不是洁癖：enqueue 跑在扇出那个 goroutine 上，它串行遍历整个频率的
// 听众。slog 的默认 handler 一路写到 stderr，而 stderr 是**会卡**的——docker 的
// json-file 驱动、journald 背压、一个写满的磁盘。日志一卡，持锁的 enqueue 跟着
// 卡，同一条会话的排空 goroutine 也拿不到锁，扇出就停在这个听众身上，整个频率
// 一起哑。那正是这条有界队列专门要消除的耦合，只是把慢的东西从"客户端的上行"
// 换成了"日志后端"。
//
// 手法是确定性的，不靠计时：hook 在写日志的那个 goroutine 上回头去调
// o.dropped()，而它要的正是 enqueue 可能还持着的那把 o.mu。sync.Mutex 不可重入，
// 所以"在锁里写日志"这个实现会当场自锁——enqueue 永远不返回，下面那个 2 秒的
// select 就是它的红。
func TestTheOverflowLogIsNotWrittenWhileTheQueueLockIsHeld(t *testing.T) {
	p := parkOutbound(t)

	// 先把队列灌满，但**不要**溢出——这几帧不该产生任何日志，装 hook 之前
	// 把它们放完，探针才只盯着溢出那一次。
	for i := 0; i < outboundDepth; i++ {
		p.enqueue(t, frame(uint16(i), 0))
	}

	h := captureLogs(t, func() {
		// 回头去拿 o.mu。日志如果是在锁里写的，这一句就是自锁。
		_ = p.o.dropped()
	})

	done := make(chan struct{})
	go func() { defer close(done); p.o.enqueue(frame(lastSeq, 0)) }()
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("enqueue never returned: the overflow line is being logged while o.mu is held, so anything that slows the log backend down (docker json-file, journald backpressure, a full disk) stalls the whole frequency's fan-out — the exact coupling this bounded queue exists to remove")
	}

	// 前提：那条日志**真的写出来了**。少了这一半，一个干脆不记日志的实现
	// 也能让上面那句话成立。
	got := h.matching("outbound queue overflowed")
	if len(got) != 1 {
		t.Fatalf("the overflow produced %d log lines, want exactly 1 — without it the assertion above is vacuous", len(got))
	}
	if got[0].Attrs["dropped_total"] != uint64(1) {
		t.Fatalf("dropped_total = %v, want 1", got[0].Attrs["dropped_total"])
	}
}

// TestAPermanentSendFailureIsReportedAndATransientOneIsNot 钉住
// `_ = o.send(p)` 那一行原本丢掉的东西。
//
// quic-go v0.48.2 的 SendDatagram 只会返回三种东西，而其中**两种不是丢包**：
// 帧超过协商出来的 datagram 上限（同样大小的每一帧都会失败），以及对端根本
// 没协商 datagram 扩展（这条会话一帧都发不出去）。两者的表现一模一样：那个
// 听众从此完全静默，而两端都没有一行日志。
//
// 第三种是连接关了，它必须**保持安静**——否则每一次正常断连都要多一行日志，
// 而那正是让人从此忽略这类日志的做法。这一行也是本表的反例：少了它，一个
// "什么错都吼一嗓子"的实现照样全绿。
func TestAPermanentSendFailureIsReportedAndATransientOneIsNot(t *testing.T) {
	for _, tc := range []struct {
		name     string
		err      error
		wantLogs int
		wantMsg  string
		wantAttr string
	}{
		{
			name:     "a frame too large for this connection",
			err:      &quic.DatagramTooLargeError{MaxDatagramPayloadSize: 1200},
			wantLogs: 1,
			wantMsg:  "cannot carry a voice frame this large",
			wantAttr: "max_datagram_payload",
		},
		{
			name:     "the peer never negotiated datagrams",
			err:      errors.New("datagram support disabled"),
			wantLogs: 1,
			wantMsg:  "cannot be sent on this connection at all",
		},
		{
			name: "the connection is simply gone",
			// quic-go 的每一种连接关闭错误的 Is() 都返回 target == net.ErrClosed
			// （internal/qerr/errors.go），所以这一个代表整族。
			err:      &quic.ApplicationError{ErrorCode: 0},
			wantLogs: 0,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			h := captureLogs(t, nil)

			var mu sync.Mutex
			calls := 0
			o := newOutbound(func([]byte) error {
				mu.Lock()
				calls++
				mu.Unlock()
				return tc.err
			})
			defer o.stop()

			// 排空 goroutine 是单线程且按序处理的，所以第 n 次 send **被调到**
			// 的时候，第 n-1 帧的汇报必然已经跑完了。等到第 5 次调用，就等于
			// 确定性地等到了前 4 次失败的全部后果——不需要 sleep 去猜。
			const frames = 5
			for i := 0; i < frames; i++ {
				o.enqueue(frame(uint16(i), 0))
			}
			deadline := time.Now().Add(waitMax)
			for {
				mu.Lock()
				n := calls
				mu.Unlock()
				if n >= frames {
					break
				}
				if time.Now().After(deadline) {
					t.Fatalf("send was called %d times, want %d — the drain goroutine never got through the queue, so nothing here is being tested", n, frames)
				}
				time.Sleep(waitStep)
			}

			errs := 0
			var sample logRecord
			for _, r := range h.matching("") {
				if r.Level >= slog.LevelError {
					errs++
					sample = r
				}
			}
			if errs != tc.wantLogs {
				t.Fatalf("%d frames failed with %v and produced %d ERROR lines, want %d — a permanent failure that says nothing leaves a silent listener with clean logs on both ends, and a transient one that says something makes every ordinary disconnect noisy",
					frames-1, tc.err, errs, tc.wantLogs)
			}
			if tc.wantLogs == 0 {
				return
			}
			if !strings.Contains(sample.Msg, tc.wantMsg) {
				t.Fatalf("the report says %q, want it to contain %q", sample.Msg, tc.wantMsg)
			}
			if tc.wantAttr != "" {
				if _, ok := sample.Attrs[tc.wantAttr]; !ok {
					t.Fatalf("the report carries %v, want an attribute %q — the number is what makes it actionable", sample.Attrs, tc.wantAttr)
				}
			}
		})
	}
}
