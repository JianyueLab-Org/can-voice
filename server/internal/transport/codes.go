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
