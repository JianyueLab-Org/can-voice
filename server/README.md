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
| `CAN_VOICE_MAX_RX` | 否 | 单会话订阅频率上限，默认 32，**上限 1024** |
| `CAN_VOICE_DEBUG` | 否 | 非空则日志降到 DEBUG |

`CAN_VOICE_MAX_RX` 有上限，是因为 `internal/router` 里 `maxRejected` 那段算术依赖它：
一份 SUBACK 要塞进 64 KiB 的控制帧，配到几千之后它就发不出去了，而那时一条**已经生效**
的 SUB 会得不到任何回应。宁可启动失败，也不要把那条静默路径留到生产上。

## QUIC 关闭码

这四个数字是**协议的一部分**，客户端靠它们决定要不要重连（定义在
`server/internal/transport/codes.go`）。

| 码 | 含义 | 客户端该怎么做 |
|---|---|---|
| 0 | 正常关闭（**进程重启部署走的就是这条**） | 可以重连 |
| 1 | 握手被拒（token 过期 / 签名不对 / 协议版本不合） | **不要原样重连**，先换一个新 token。原样重试只会得到同一个答复，而 can-api 对失败按 CID 限流 |
| 2 | 同一个 CID 在别处登录，这一条被顶掉（原因串 `evicted`） | **必须停止重连**，并告诉用户账号在别处登录了 |
| 3 | 协议违规（见原因串 `control_write_stalled` / `control_read_stalled` / `ack_undeliverable`） | **不要重连**，去修客户端的控制流读写 |

码 2 那条要特别说清楚：自动重连会顶掉刚刚顶掉自己的那条会话，对方再重连再顶回来，
两个客户端无限互顶。"连续失败三次就放弃"挡不住——顶号后的重连是**成功**的，
一成功计数器就清零。

码 3 是同一个形状的另一个版本：重连会立刻把同一个 bug 再演一遍，于是拒绝—重连—
再拒绝，无限循环；而重连本身是**成功**的，所以有界重连那套计数器同样清零。这也
正是它不能复用码 0 的理由——码 0 的含义是"你可以按自己的策略重连"，而在这里
重连恰恰是错的答案。

### 关闭原因串

关闭码 1 太粗：客户端需要知道该换 token 还是该修代码。原因串装在
`ApplicationError.ErrorMessage` 里，它与关闭码**原子地一起送达**，
所以这是唯一一条丢不掉的通道（BYE 会丢——一个还没开始读的客户端收不到它）。
常量定义在 `server/internal/transport/codes.go`。

| 原因串 | 含义 | 客户端该怎么做 |
|---|---|---|
| `token_expired` | token 本身没问题，只是过期了 | 去 can-api 换一张新的，然后重连 |
| `token_invalid` | 形状、签名或内容不对 | **不要重试**，这是客户端或签发方的 bug |
| `refused` | 这条消息不该出现在这里（比如 HELLO 之前先发了别的）、`HELLO.proto` 不是本服务端讲的版本、或者 `follow` 不是一个合法呼号 | 同上，协议用错了；版本不合要**升级客户端**，换 token 没有用 |
| `evicted` | 同一个 CID 在别处登录，这一条被顶掉。配关闭码 2 | **必须停止重连**，并告诉用户账号在别处登录了 |
| `control_write_stalled` | 握手**之后**，服务端往控制流写一帧超过 10 秒没写出去——也就是客户端不再读这条流了 | **不要重连**，去修控制流的读取。配关闭码 3 |
| `control_read_stalled` | 握手**之后**，客户端开了一帧（长度前缀已经发了）却在 10 秒内没把它发完 | **不要重连**，去修控制流的写入。配关闭码 3 |
| `ack_undeliverable` | 一份 `SUB` **已经生效**，但它的 `SUBACK` 大到发不出去 | **不要重连**，把声明改小。配关闭码 3 |

这七个串和四个码一样是**协议**：客户端按字面值判断，改动等于改协议。`evicted`
曾经是一句内联的英文散文，而它挂在**唯一一个客户端必须遵守的关闭码**上——
严格说靠码 2 就够判了，但一个既看码又看串的客户端是完全合理的写法，散文被人
顺手改一改措辞就把它弄坏了，还没有任何东西会响。

`HELLO.proto` 是被**真的检查**的：它必须等于 `control.ProtoVersion`（当前是 1），
缺席（也就是 0）同样被拒。ALPN（`can-voice/1`）和它说的是同一件事的两层——
ALPN 约束"这条连接讲的是哪个协议"，`proto` 是客户端自己声明的控制面版本，
一个把 ALPN 抄对、却按旧版控制面编消息的客户端会在这里被挡住。

`HELLO.follow` 只有观察员模式填，规则照抄 can-fsd 的 `IsValidCallsign`：
2–10 个字符，只许 `A-Z`、`0-9`、`-`、`_`。它是拿去 datafeed 里按呼号查位置的键，
比这条规则松只会放进永远查不到的值，紧则会把合法呼号挡在外面。

`control_read_stalled` 是 `control_write_stalled` 的读侧版本：一个发了长度前缀就不发完
的对端能永久占住一条会话和三个 goroutine，而 QUIC 的空闲超时救不了——包**确实在到达**，
每一个都会把空闲计时器重置。**一个字节都不发不算**：几分钟不说话的管制员完全正常，
那种连接上没有开始过任何一帧。分界线是"你已经承诺了一帧"。

`ack_undeliverable` 正常情况下碰不到：声明本身有上界，回报也各自有上界。它是一条防线——
不这么关的话，服务端会以码 0（"你可以重连"）收场，而客户端的订阅状态和服务端的已经对不上，
于是它重连、重放同一份声明，再一次静默失败。

`control_write_stalled` 和前三个不同：它不是握手被拒的原因，而是一条已建立会话的
死因。它存在是因为读和写在同一个 goroutine 里——一个不读控制流的客户端会让服务端
的写卡在 QUIC 的流控上，于是服务端也**不再读**，客户端之后的 `SUB` 一条都不会被
处理。症状是最糟的那种：客户端继续收着旧的那套频率，台面改动看上去毫无反应，
而连接一切正常。空闲超时救不了这件事——它在收到任何报文时都会重置，而保活 PING
会被对端的传输层自动 ACK，跟应用读不读无关。

## 运行

    go build -o can-voice ./server/cmd/can-voice
    CAN_VOICE_ADDR=:64738 \
    CAN_VOICE_TLS_CERT=/etc/letsencrypt/live/audio.ceruleanavi.net/fullchain.pem \
    CAN_VOICE_TLS_KEY=/etc/letsencrypt/live/audio.ceruleanavi.net/privkey.pem \
    CAN_VOICE_API_PUBKEY=$(cat /etc/can-voice/api.pub) \
    ./can-voice

## 排障

- **所有人都连不上，日志里是 `token signature does not verify`**：`CAN_VOICE_API_PUBKEY`
  与 can-api 的私钥不配对。
- **所有人都连不上，日志里是 `token expired`**：这**不是**密钥的问题，是**时钟**的问题。
  can-api 签的是 60 秒的 token，服务端的时钟只要比真实时间快过那么多，每一张刚签出来的
  票一到这里就已经过期，于是全网被以 `token_expired` 拒掉——而客户端照着这个原因串去
  换新票，换回来的还是过期的。`internal/auth` 容忍几秒的偏差，超过就要去修 NTP。
  对照一下服务端时间和 can-api 的时间，别去动公钥。
- **某个人连不上，别人都正常，日志里是 `rating is below the minimum`**：这不是故障。
  未定级成员（rating < 1）不能用语音，和 can-api 的 `/api/v1/public/auth` 同一条规则。
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
