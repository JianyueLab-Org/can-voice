package transport

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/tls"
	"testing"
	"time"

	"github.com/JianyueLab-Org/can-voice/server/internal/auth"
	"github.com/JianyueLab-Org/can-voice/server/internal/control"
	"github.com/JianyueLab-Org/can-voice/server/internal/router"
)

// 本文件钉的是握手里的**准入**：谁能进来，以及进来之后他自己那张票说了多少算。
// 两件事都只有在 token 完全合法的前提下才有意义——拿一张坏 token 什么都证明
// 不了，它在验签那一步就被拒了。

// TestAnUnratedMemberIsRefusedEvenWithAPerfectlyGoodToken 钉住等级这道闸。
//
// can-api 的 /api/v1/public/auth——这个网络上其它每一个组件用的那道凭据检查——
// 明文拒绝 rating < 1，can-audio 的文档也写着"未定级成员即便凭据正确也不能用
// 语音"。少了这一条，can-voice 就是全网唯一一个放未定级成员进来的入口，而且
// 从外面完全看不出来：token 是真的，签名是对的，日志一切正常。
func TestAnUnratedMemberIsRefusedEvenWithAPerfectlyGoodToken(t *testing.T) {
	addr, priv, r := testServer(t)
	tok, err := auth.Sign(priv, auth.Claims{
		CID: "1000", Rating: 0, MaxTX: 8, Exp: time.Now().Add(time.Minute).Unix(),
	})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}

	before := r.SessionCount()
	_, m := hello(t, dial(t, addr), tok)
	bye, ok := m.(*control.Bye)
	if !ok {
		t.Fatalf("an unrated member got %T, want *control.Bye — can-api refuses rating < 1 and voice must not be the one door that does not", m)
	}
	// "refused" 而不是 token_expired / token_invalid：那两个的意思都是"去换一张
	// 票再来"，而换票不会给人升级——照着它做的客户端会一直重试到被限流。
	if bye.Reason != ReasonRefused {
		t.Fatalf("Reason = %q, want %q — telling an unrated member to fetch a new token sends it into a loop that cannot succeed", bye.Reason, ReasonRefused)
	}
	if got := r.SessionCount(); got != before {
		t.Fatalf("sessions = %d, want %d — a refused member must not be left registered", got, before)
	}
}

// TestAMemberAtTheMinimumRatingIsAdmitted 是上一条的对照。
//
// 一个把**所有人**都拒掉的变异体，光靠"应该被拒"那一条是抓不住的。输入取在
// 闸门的边界上（rating == minRating），所以它同时也钉住闸是 `< minRating`
// 而不是 `<= minRating`。
func TestAMemberAtTheMinimumRatingIsAdmitted(t *testing.T) {
	addr, priv, _ := testServer(t)
	tok, err := auth.Sign(priv, auth.Claims{
		CID: "1000", Rating: minRating, MaxTX: 8, Exp: time.Now().Add(time.Minute).Unix(),
	})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	_, m := hello(t, dial(t, addr), tok)
	if _, ok := m.(*control.Ready); !ok {
		t.Fatalf("a member at the minimum rating got %T, want *control.Ready — the gate must be rating < %d, not rating <= %d", m, minRating, minRating)
	}
}

// TestAnOversizedMaxTXClaimIsClampedByTheServer 钉住 MaxTX 也要被服务端夹一次。
//
// MaxTX 是唯一一个直接来自对端的**资源上限**（MaxRX 是服务端配置，CID/Rating
// 是身份）。session.go 自己写着"已鉴权不等于可信"，而在这之前 MaxTX 正是那句话
// 唯一没管住的字段。
//
// 两半都要断言：READY 里回显的数字必须是夹过的（否则客户端照着一个服务端不会
// 兑现的数字画界面），而且真的只有那么多频率能发射（否则那个数字只是一句好听
// 的话——TestMaxTXIsEnforcedOverTheWire 的注释已经说过只断言回显等于什么都没
// 断言）。
func TestAnOversizedMaxTXClaimIsClampedByTheServer(t *testing.T) {
	addr, priv, r := testServer(t) // testServer 的 MaxRX 是 32
	tok, err := auth.Sign(priv, auth.Claims{
		CID: "1000", Rating: 5, MaxTX: 10000, Exp: time.Now().Add(time.Minute).Unix(),
	})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	st, m := hello(t, dial(t, addr), tok)
	ready, ok := m.(*control.Ready)
	if !ok {
		t.Fatalf("got %T, want *control.Ready", m)
	}
	if ready.MaxTX != serverMaxTX {
		t.Fatalf("READY.MaxTX = %d, want the server's own cap %d — echoing the token's number back tells the client it may do something the server will refuse", ready.MaxTX, serverMaxTX)
	}

	tx := make([]uint32, 0, serverMaxTX+8)
	for i := 0; i < serverMaxTX+8; i++ {
		tx = append(tx, uint32(118000+i*25))
	}
	ack := subAckFor(t, st, control.Sub{TX: tx})
	if len(ack.TX) != serverMaxTX {
		t.Fatalf("accepted TX = %d, want %d — a token claiming max_tx=10000 must not actually get 10000 transmit frequencies", len(ack.TX), serverMaxTX)
	}
	id := router.SessionID(ready.Session)
	for _, f := range ack.Rejected {
		if r.MayTransmit(id, f) {
			t.Fatalf("%d was rejected in the ACK but the router still lets the session transmit on it", f)
		}
	}
	// 缺席断言的前提：被接受的那些确实能发，否则上面那句可能只是因为这条会话
	// 在**任何**频率上都不能发。
	for _, f := range ack.TX {
		if !r.MayTransmit(id, f) {
			t.Fatalf("premise failed: %d was granted in the ACK but the router refuses it too", f)
		}
	}
}

// TestMaxTXIsAlsoClampedToMaxRX 是同一道闸的另一半，也是它真正堵住的那条
// 绕过路径。
//
// TX ⊆ RX，而且 TX 频率**不受 MaxRX 挤压**（router 里那个 `already` 判断，是有意
// 的设计）。所以一条会话能进倒排索引的频率数其实是 max(MaxTX, MaxRX)：不把 MaxTX
// 夹到 MaxRX 以内的话，一张 max_tx 很大的 token 想登记多少频率就登记多少，而
// MaxRX 这个服务端配置根本拦不住它。
//
// 输入必须落在 MaxRX < serverMaxTX 这一侧。默认的 MaxRX=32 下是 serverMaxTX 先
// 生效，把 min 里的 MaxRX 那一项整个删掉也照样绿——这正是"一个断言只有在它的
// 输入能区分两种实现时才会失败"。
func TestMaxTXIsAlsoClampedToMaxRX(t *testing.T) {
	const maxRX = 3

	pub, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatalf("GenerateKey: %v", err)
	}
	cert, err := SelfSignedCert()
	if err != nil {
		t.Fatalf("SelfSignedCert: %v", err)
	}
	r := router.New()
	cfg := Config{
		Addr:      "127.0.0.1:0",
		TLS:       &tls.Config{Certificates: []tls.Certificate{cert}},
		PublicKey: pub,
		MaxRX:     maxRX,
	}
	ln, err := listen(cfg)
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(func() { cancel(); ln.Close() })
	go accept(ctx, ln, cfg, r, newConnSet())

	tok, err := auth.Sign(priv, auth.Claims{
		CID: "1000", Rating: 5, MaxTX: serverMaxTX, Exp: time.Now().Add(time.Minute).Unix(),
	})
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	st, m := hello(t, dial(t, ln.Addr().String()), tok)
	ready, ok := m.(*control.Ready)
	if !ok {
		t.Fatalf("got %T, want *control.Ready", m)
	}
	if ready.MaxTX != maxRX {
		t.Fatalf("READY.MaxTX = %d with MaxRX = %d — max_tx above max_rx is exactly the path that goes around the RX limit, because TX frequencies are exempt from it", ready.MaxTX, maxRX)
	}

	ack := subAckFor(t, st, control.Sub{TX: []uint32{118000, 118025, 118050, 118075, 118100, 118125}})
	if len(ack.TX) != maxRX {
		t.Fatalf("accepted TX = %d, want %d", len(ack.TX), maxRX)
	}
	if len(ack.RX) > maxRX {
		t.Fatalf("the session is indexed on %d frequencies with MaxRX = %d; TX-implied RX went around the configured limit", len(ack.RX), maxRX)
	}
}

// subAckFor 发一份声明并读回 SUBACK，不校验内容。
//
// 刻意不用 helpers_test.go 的 client.subscribe：那个会把任何 Rejected 判成
// Fatal，而本文件里每一条测试的重点恰恰**就是**有东西被拒。
func subAckFor(t *testing.T, st interface {
	Read([]byte) (int, error)
	Write([]byte) (int, error)
}, sub control.Sub) control.SubAck {
	t.Helper()
	b, err := control.Encode(&sub)
	if err != nil {
		t.Fatalf("Encode SUB: %v", err)
	}
	if err := control.WriteFrame(st, b); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	raw, err := control.ReadFrame(st)
	if err != nil {
		t.Fatalf("ReadFrame (SUBACK): %v", err)
	}
	m, err := control.Decode(raw)
	if err != nil {
		t.Fatalf("Decode SUBACK: %v", err)
	}
	ack, ok := m.(*control.SubAck)
	if !ok {
		t.Fatalf("after SUB got %T, want *control.SubAck", m)
	}
	return *ack
}
