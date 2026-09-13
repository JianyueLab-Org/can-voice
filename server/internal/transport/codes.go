package transport

import "github.com/quic-go/quic-go"

// QUIC 应用层关闭码。这三个数字是**协议的一部分**：客户端靠它们决定要不要重连，
// 所以改动等于改协议，不是重构。
//
// 客户端契约（写在这里，因为指望每个客户端作者自己想明白是行不通的）：
//
//	CloseNormal           → 可以按自己的策略重连。
//	CloseHandshakeRefused → 先换一张新 token，否则不要重连。
//	CloseEvicted          → **不要重连**，并告诉用户"你的账号在别处登录了"。
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
)
