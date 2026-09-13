# can-voice 服务端

QUIC 语音扇出服务。无状态：没有数据库、没有持久化、没有 ACL。

## 配置

全部环境变量，缺任何必需项启动即失败。

| 变量 | 必需 | 说明 |
|---|---|---|
| `CAN_VOICE_ADDR` | 是 | UDP 监听地址，如 `:64738` |
| `CAN_VOICE_TLS_CERT` | 是 | 证书链 PEM（Let's Encrypt） |
| `CAN_VOICE_TLS_KEY` | 是 | 私钥 PEM |
| `CAN_VOICE_API_PUBKEY` | 是 | can-api 的 Ed25519 公钥，裸 32 字节的 base64 |
| `CAN_VOICE_FSD_FEED` | 否 | can-fsd 的 SSE，默认 `https://data.ceruleanavi.net/v1/events` |
| `CAN_VOICE_MAX_RX` | 否 | 单会话订阅频率上限，默认 32 |
| `CAN_VOICE_DEBUG` | 否 | 非空则日志降到 DEBUG |

## QUIC 关闭码

这三个数字是**协议的一部分**，客户端靠它们决定要不要重连（定义在
`server/internal/transport/codes.go`）。

| 码 | 含义 | 客户端该怎么做 |
|---|---|---|
| 0 | 正常关闭 | 可以重连 |
| 1 | 握手被拒（token 过期 / 签名不对 / 协议版本不合） | **不要原样重连**，先换一个新 token。原样重试只会得到同一个答复，而 can-api 对失败按 CID 限流 |
| 2 | 同一个 CID 在别处登录，这一条被顶掉 | **必须停止重连**，并告诉用户账号在别处登录了 |

码 2 那条要特别说清楚：自动重连会顶掉刚刚顶掉自己的那条会话，对方再重连再顶回来，
两个客户端无限互顶。"连续失败三次就放弃"挡不住——顶号后的重连是**成功**的，
一成功计数器就清零。

### 关闭原因串

关闭码 1 太粗：客户端需要知道该换 token 还是该修代码。原因串装在
`ApplicationError.ErrorMessage` 里，它与关闭码**原子地一起送达**，
所以这是唯一一条丢不掉的通道（BYE 会丢——一个还没开始读的客户端收不到它）。
常量定义在 `server/internal/transport/codes.go`。

| 原因串 | 含义 | 客户端该怎么做 |
|---|---|---|
| `token_expired` | token 本身没问题，只是过期了 | 去 can-api 换一张新的，然后重连 |
| `token_invalid` | 形状、签名或内容不对 | **不要重试**，这是客户端或签发方的 bug |
| `refused` | 这条消息不该出现在这里（比如 HELLO 之前先发了别的） | 同上，协议用错了 |

这三个串和三个码一样是**协议**：客户端按字面值判断，改动等于改协议。

## 运行

    go build -o can-voice ./server/cmd/can-voice
    CAN_VOICE_ADDR=:64738 \
    CAN_VOICE_TLS_CERT=/etc/letsencrypt/live/audio.ceruleanavi.net/fullchain.pem \
    CAN_VOICE_TLS_KEY=/etc/letsencrypt/live/audio.ceruleanavi.net/privkey.pem \
    CAN_VOICE_API_PUBKEY=$(cat /etc/can-voice/api.pub) \
    ./can-voice

## 排障

- **所有人都连不上**：`CAN_VOICE_API_PUBKEY` 与 can-api 的私钥不配对，日志里每条
  会是 `token signature does not verify`。
- **管制员谁都听不见**：位置解析问题。can-fsd 的 datafeed 里管制员的经纬度是 JSON
  **字符串**而飞行员的是数字；解析错会让管制员全部落在 0,0。
  `internal/fsdfeed` 的 `TestControllerCoordinatesParseFromStrings` 钉着这条。
- **射程完全不起作用**：检查日志里有没有 `fsd feed dropped`。SSE 断开时服务端刻意
  降级为不做射程过滤——语音能不能通比射程真实感重要得多。
- **启动时报 `CAN_VOICE_API_PUBKEY decodes to 44 bytes`**：贴进来的是 DER 包装的
  公钥（`openssl pkey -pubout` 默认导出的那种，以 `MCowBQYDK2Vw` 开头），本服务
  只收**裸 32 字节**的 base64。转换：

      openssl pkey -pubin -in api.pub.pem -outform DER | tail -c 32 | base64

- **`Restart=always` 反复重启且日志里是权限错误**：读不到私钥。Let's Encrypt 在
  Debian 上默认把 `live/*/privkey.pem` 放成 `root:root 0600`，而单元里的
  `DynamicUser=yes` 拿到的是一个动态 uid。要么把私钥给 `ssl-cert` 组
  （`chgrp ssl-cert` + `chmod 640`，并让 certbot 的 deploy hook 每次续期后重做），
  要么去掉 `DynamicUser` 改用一个固定账号。这一条不会在测试里出现，只会在
  **续期那天**出现——证书换了新文件，权限回到默认值。
