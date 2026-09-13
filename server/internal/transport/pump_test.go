package transport

import (
	"bytes"
	"context"
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/control"
	"github.com/JianyueLab-Org/can-voice/server/internal/router"
	"github.com/JianyueLab-Org/can-voice/server/internal/wire"
)

// 本文件钉住数据面这条泵：上行 datagram → Fanout，以及 Session.Send → 下行 datagram。
//
// 这两条在复审之前**一条测试都没有**，而它们正是这个包存在的理由。实测的后果：
// 把 `go readDatagrams(...)` 整行删掉、让 Fanout 永远不被调用、把 Send 回调改成
// 空函数——三种改法各自都让服务端彻底不传音频，而原来那 15 条测试一条都不红。
// 一个"全绿但没有声音"的服务端是这个项目最不能出的东西。
//
// 拆成两条而不是一条，是为了让红的时候能定位：只有第一条红 = readDatagrams 或
// Fanout 断了；两条都红 = Send 回调没接到 conn.SendDatagram 上。

// TestASpokenDatagramReachesASubscribedListener 是上行半边：
// 一个客户端发出的 datagram 必须真的走到 Fanout，并按订阅表投出去。
func TestASpokenDatagramReachesASubscribedListener(t *testing.T) {
	addr, priv, _ := testServer(t)

	speaker := connect(t, addr, "1000", 8, priv)
	listener := connect(t, addr, "1001", 8, priv)

	// subscribe 自己会把"被拒"变成 Fatal，所以下面"听见了"的断言不会因为
	// 订阅其实没生效而假绿。
	speaker.subscribe(t, control.Sub{TX: []uint32{121800}})
	listener.subscribe(t, control.Sub{RX: []uint32{121800}})

	opus := []byte{0x01, 0x02, 0x03, 0x04}
	pkt := append(wire.Header{
		Ver: wire.Version, Flags: wire.FlagFirst, Seq: 7, FreqKHz: 121800,
	}.AppendTo(nil), opus...)

	if err := speaker.conn.SendDatagram(pkt); err != nil {
		t.Fatalf("SendDatagram: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	got, err := listener.conn.ReceiveDatagram(ctx)
	if err != nil {
		t.Fatalf("the listener heard nothing: %v — the inbound datagram never reached the router's fan-out", err)
	}

	h, payload, err := wire.Parse(got)
	if err != nil {
		t.Fatalf("Parse: %v", err)
	}
	if h.FreqKHz != 121800 {
		t.Fatalf("FreqKHz = %d, want 121800", h.FreqKHz)
	}
	if h.Speaker != speaker.sess {
		t.Fatalf("Speaker = %d, want the speaker's session id %d — the receiver tells two people apart by this field", h.Speaker, speaker.sess)
	}
	// 载荷必须原样过去：服务端没有 Opus 依赖，音频对它是不透明字节。
	if !bytes.Equal(payload, opus) {
		t.Fatalf("payload = %v, want %v unchanged", payload, opus)
	}
}

// TestTheSendCallbackReachesThePeerAsADatagram 是下行半边，单独钉。
//
// 它不经过 Fanout：直接从 router 取出这条会话，调它的 Send。上一条测试同时依赖
// 上行和下行，这一条只依赖"SessionOpts.Send 真的接在 conn.SendDatagram 上"，
// 所以两条一起看就能把故障分到哪一边。
func TestTheSendCallbackReachesThePeerAsADatagram(t *testing.T) {
	addr, priv, r := testServer(t)
	c := connect(t, addr, "1000", 8, priv)

	sess, ok := r.Get(router.SessionID(c.sess))
	if !ok {
		t.Fatal("premise: the handshake did not register the session with the router")
	}

	// 刻意不是一个合法的 wire 包：Send 这一层不解析，也不该解析。
	want := []byte{0xde, 0xad, 0xbe, 0xef}
	sess.Send(want)

	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	got, err := c.conn.ReceiveDatagram(ctx)
	if err != nil {
		t.Fatalf("the peer received nothing: %v — SessionOpts.Send is not wired to conn.SendDatagram", err)
	}
	if !bytes.Equal(got, want) {
		t.Fatalf("datagram = %v, want %v", got, want)
	}
}

// TestClosingAConnectionRemovesItsSessionFromTheRouter 钉住拆连接时会话真的被摘掉。
//
// 不摘的话，死会话永远留在 Listeners(freq) 里，全网每一次重连都往那条频率上
// 再加一个幽灵听众——扇出对着它们逐个算射程、逐个调一个指向已关连接的 Send。
// 队列化出站（已排期）会让每个幽灵再多背一份常驻内存。
func TestClosingAConnectionRemovesItsSessionFromTheRouter(t *testing.T) {
	addr, priv, r := testServer(t)
	c := connect(t, addr, "1000", 8, priv)
	c.subscribe(t, control.Sub{RX: []uint32{118000}})

	// 前提：它确实进了扇出索引。不证的话，"最后它不在索引里"可能只是因为
	// 它从头到尾就没进去过。
	if len(r.Listeners(118000)) != 1 {
		t.Fatal("premise: the subscription never reached the router's index")
	}

	c.conn.CloseWithError(CloseNormal, "client done")

	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		if len(r.Listeners(118000)) == 0 {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("the session is still a listener on 118000 after its connection closed; every reconnect on the network leaves another phantom behind")
}
