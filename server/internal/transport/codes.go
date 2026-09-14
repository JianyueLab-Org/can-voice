package transport

import "github.com/quic-go/quic-go"

// QUIC 应用层关闭码。这四个数字是**协议的一部分**：客户端靠它们决定要不要重连，
// 所以改动等于改协议，不是重构。
//
// 客户端契约（写在这里，因为指望每个客户端作者自己想明白是行不通的）：
//
//	CloseNormal            → 可以按自己的策略重连。
//	CloseHandshakeRefused  → 先换一张新 token，否则不要重连。
//	CloseEvicted           → **不要重连**，并告诉用户"你的账号在别处登录了"。
//	CloseProtocolViolation → **不要重连**，去修客户端。
const (
	// CloseNormal 是正常关闭：进程退出、客户端主动断开。客户端可以重连。
	CloseNormal quic.ApplicationErrorCode = 0

	// CloseHandshakeRefused 是握手被拒：token 过期、签名不对、协议版本不合。
	// 客户端**重连也没用**，除非先换一个新 token——原样重试只会得到同一个答复，
	// 而 can-api 那边对失败是按 CID 限流的。
	//
	// 被拒的原因在连接关掉之前会先以一条 BYE 发出去（reasonFor 的两级粗粒度
	// 代码），客户端应当读那一条来决定"去换 token"还是"别试了"。
	CloseHandshakeRefused quic.ApplicationErrorCode = 1

	// CloseEvicted 是同一个 CID 在别处登录，这一条被顶掉了。
	//
	// **客户端收到这个码必须停止重连，并告诉用户"你的账号在别处登录了"。**
	// 自动重连会顶掉刚刚顶掉自己的那条会话，对方再重连再顶回来，两个客户端
	// 无限互顶。注意"连续失败三次就放弃"那条有界重连策略挡不住这件事：
	// 顶号之后的重连是**成功**的，一成功计数器就清零。
	//
	// 顶号路径上刻意不发 BYE：被顶的那一条随时可能正卡在读上，而关闭码
	// 一定到得了对端（它就在 CONNECTION_CLOSE 帧里）。
	CloseEvicted quic.ApplicationErrorCode = 2

	// CloseProtocolViolation 是对端把协议用坏了，坏到这条连接没法再继续。
	// 今天只有一种情况会走到这里：控制流的写被对端按住超过了
	// controlWriteTimeout（原因串 ReasonControlWriteStalled）。
	//
	// **客户端收到这个码必须去修自己的控制流读取，而不是重连。** 重连会立刻
	// 把同一个 bug 再演一遍，于是拒绝—重连—再拒绝，无限循环——和 CloseEvicted
	// 那个互顶形状是同一类，而且"连续失败三次就放弃"同样挡不住：重连本身是
	// **成功**的，计数器一成功就清零，真正的失败发生在几秒之后。
	//
	// 这也正是它不能复用 CloseNormal 的理由：那个码的含义是"你可以按自己的
	// 策略重连"，而在这里重连恰恰是错的答案。
	CloseProtocolViolation quic.ApplicationErrorCode = 3
)

// 握手被拒的原因。和上面三个关闭码一样，这几个字符串是**协议的一部分**：
// 客户端靠它们决定"去换一张新 token 再试"还是"别试了"，改动等于改协议。
//
// 它们走**两条**路出去，而且以第二条为准：
//
//  1. 一条 BYE 控制帧，尽力而为。
//  2. QUIC CONNECTION_CLOSE 的 reason phrase，也就是
//     `conn.CloseWithError(CloseHandshakeRefused, <这里>)`。
//
// 必须以第二条为准，因为**第一条根本不可靠**：quic-go 收到 CONNECTION_CLOSE 之后
// 会把已经到达、还没被应用读走的流数据整个丢掉（receive_stream.go 的 readImpl
// 先查 closeForShutdownErr，查到就返回错误，不看缓冲区）。一个建连之后没有立刻
// 去读控制流的客户端——完全正常的写法——会连一个字都收不到。而关闭码和它的
// reason phrase 是同一个帧里的同一件事，**原子地**送达（connection.go:1364-1368），
// 丢不掉。
//
// 所以 BYE 只是给已经在读的客户端的一点方便，判断要拿 ApplicationError.ErrorMessage。
const (
	// ReasonTokenExpired：token 本身没问题，只是过期了。
	// **这是唯一值得重试的失败**，而且必须先去换一张新 token。
	ReasonTokenExpired = "token_expired"

	// ReasonTokenInvalid：token 的形状、签名或内容不对。别重试——
	// 原样重试只会得到同一个答复，而 can-api 对失败是按 CID 限流的。
	ReasonTokenInvalid = "token_invalid"

	// ReasonRefused：这条消息就不该出现在这里（比如 HELLO 之前先发了别的），
	// 或者服务端自己配置有问题。同样别重试。
	//
	// 刻意粗：对端在这一刻**还没有通过鉴权**，"是签名长度不对还是 payload 不是
	// 合法 JSON"只对伪造 token 的人有用。详细原因留在服务端日志里。
	ReasonRefused = "refused"

	// ReasonProtoUnsupported：HELLO 里的 proto 不是本服务端讲的控制面版本。
	// 配 CloseHandshakeRefused。
	//
	// 动作和 ReasonRefused 一样（都别原样重试），**但对人说的话不一样**，
	// 这就是它值得单独占一个串的全部理由：这个网络的客户端是装在成员机器上的
	// 桌面程序，一个版本太旧的用户该看到"请更新客户端"。告诉他"被拒绝"会把他
	// 送去查密码、去 can-api 换票、去问自己的账号是不是出了问题——那三件事
	// 一件都帮不上忙，他客户端目录里那个旧 exe 才是原因。
	//
	// 上一轮把它映射到 ReasonRefused，那与当时的文档一致因而不算错；但那份
	// 文档写的是"两级粗粒度代码"，而那两级分的是"去换票"和"别试了"——
	// 版本太旧属于第三种：**去更新**。粗粒度的理由（对端尚未鉴权，细节只帮
	// 伪造者）在这里不成立：proto 是客户端自己声明的，告诉他他自己说了什么
	// 不合用，没有泄露任何东西。
	//
	// 它和 ALPN 是同一件事的两层：ALPN 对不上时 TLS 握手就失败了，根本走不到
	// 这里；这一条挡的是"ALPN 抄对了、控制面却按旧版编"的客户端。
	ReasonProtoUnsupported = "proto_unsupported"

	// ReasonEvicted：同一个 CID 在别处登录，这一条被顶掉了。配 CloseEvicted。
	//
	// 它此前是一句内联的英文句子（"another session signed in with this account"）
	// ——而它挂在**客户端唯一一个必须遵守的关闭码**上：收到码 2 必须停止重连，
	// 否则两个客户端无限互顶（见 CloseEvicted）。一个要靠字面值判断的东西写成
	// 一句散文，等于邀请下一个人顺手改改措辞，而那会把某个客户端的判断悄悄弄坏。
	//
	// 严格说客户端靠码 2 就够判了，原因串是给日志和给用户看的那句话用的。
	// 但"能靠码判"不是"可以随便改串"的理由：一个既看码又看串的客户端是完全
	// 合理的写法，而它坏掉的时候没有任何东西会响。
	ReasonEvicted = "evicted"

	// ReasonControlWriteStalled：握手**之后**，服务端往控制流写一帧的时间超过了
	// controlWriteTimeout——也就是对端不再读这条流了。配 CloseProtocolViolation。
	//
	// 这条和上面三条不同：它不是握手被拒的原因，而是一条已建立会话的死因。放在
	// 同一组常量里，是因为它走的是同一条通道（CONNECTION_CLOSE 的 reason phrase），
	// 客户端也用同一种方式读它。
	//
	// 为什么不能只是"重试这次写"：control.WriteFrame 是长度前缀加载荷两次 Write，
	// 超时可能正好落在两次之间，这条流已经不同步了，没有任何可以续下去的东西。
	//
	// 症状比"连接断了"更值得写下来：写被按住时 readControl 也就**不再读**了——
	// 读和写在同一个 goroutine 里。于是客户端之后的 SUB 一条都不会被处理，它
	// 继续收着旧的那套频率，而它的无线电台面改动看上去毫无反应，连接却一切正常。
	// 一个管制员重排台面却悄悄没生效，正是这套重写要躲开的那类故障。
	ReasonControlWriteStalled = "control_write_stalled"

	// ReasonControlReadStalled：握手**之后**，对端开了一帧（长度前缀已经到了）
	// 然后不把它发完，超过了 controlReadTimeout。配 CloseProtocolViolation。
	//
	// 和上一条是同一个形状的读侧版本：一条永远发不完的帧能占住一条会话和它的
	// 三个 goroutine，而 QUIC 的空闲超时救不了——包**确实在到达**，每一个都会
	// 把空闲计时器重置。
	//
	// **一个字节都不发不算这一条**：几分钟不说话的管制员是正常的，那种连接上
	// 没有开始过任何一帧。分界线是"你已经承诺了一帧"。
	ReasonControlReadStalled = "control_read_stalled"

	// ReasonAckUndeliverable：一份 SUB **已经生效**，但它的 SUBACK 发不出去
	// （编码失败，或者超过 64 KiB 的帧上限）。配 CloseProtocolViolation。
	//
	// 为什么不能沿用 CloseNormal：那个码说"你可以重连"，而客户端的订阅状态此刻
	// 和服务端的对不上。它会重连、重放同一份声明，再一次得到同样的静默——
	// 两端都没有一个字的死循环，正是这套协议在别处反复拒绝的失败形态。
	//
	// 正常情况下客户端碰不到它：声明本身有上界（router 的 declarationLimit），
	// 回报也各自有上界（maxRejected、maxXCPairs）。它是一条防线，不是一条日常
	// 路径——但防线要出声，否则它在的时候和不在的时候看起来一模一样。
	ReasonAckUndeliverable = "ack_undeliverable"
)
