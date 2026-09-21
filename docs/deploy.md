# can-voice 部署

语音层怎么上线。变量、线协议和排障仍以 [`server/README.md`](../server/README.md) 为准；测通不通用 [`manual-test.md`](manual-test.md)。

**现在不要停 Murmur。** 测通之前 64738 仍给老语音。切流是全网一件事：停 Murmur、can-voice 接到同一 UDP 端口。两边不能同时占。

## 跑在哪

| 东西 | 放哪 | 不放哪 |
|---|---|---|
| 语音服务端 `can-voice` | 一台能收 **UDP 64738** 的机器，Docker Compose | **不进 jyl-tyo**。集群入口是 Cloudflare Tunnel，过不了 QUIC |
| 通播机队 `can-voice-atis` | 同一份 compose 的第二个容器 | 不要和语音服务端打成一个镜像 |
| 换票 | 现网 **can-api**（`VOICE_TOKEN_KEY`） | 语音机不持私钥 |
| 四个桌面客户端 | GitHub Release 安装包 | 不部署到服务器 |

镜像由 CI 推到 GHCR：`ghcr.io/jianyuelab-org/can-voice`（amd64 + arm64）和 `…/can-voice-atis`（只 amd64）。仓库私有，包默认也私有，拉之前要登录。

64738 不是协议要求，是 Mumble 的默认端口。切流时占住它，老客户端（主机写死 `audio.ceruleanavi.net`）会握手失败，而不是连进一个空的 Murmur。测、或不碰 Murmur 时，用 `CAN_VOICE_PORT` 映射到别的宿主机端口即可。

## 一辈子一次：密钥对

在 **can-api** 仓库：

```bash
go run ./cmd/voice-keygen
```

一次印出两半，标签已经写好该进哪个变量：

| 这一半 | 放哪儿 | 干什么 |
|---|---|---|
| `VOICE_TOKEN_KEY` | can-api 的环境变量（见 `can-api/deploy/k8s.yaml`） | **签**票。这就是全部权限 |
| `CAN_VOICE_API_PUBKEY` | 语音机 `server/.env` | **验**票。泄露无所谓 |

两半放反：全网连不上，日志 `token signature does not verify`。密钥空着：can-api **不注册** `POST /api/v1/voice/token`，客户端换票失败。

公钥必须是裸 32 字节的 base64。`openssl pkey -pubout` 那种 DER 包装（以 `MCowBQYDK2Vw` 开头）启动就会报错，转换见 `server/README.md`《排障》。

改完 can-api 要让现网进程吃到新变量。**不要重新生成一对来「换机器」**——公钥照抄过去；重做密钥就要两边同时改，改完之前全网连不上。

## 每次换机器：域名、证书、compose、防火墙

### 1. 域名和证书

`audio.ceruleanavi.net` 的 A/AAAA 指到这台机器。

```bash
ss -lntp | grep ':80 '          # certbot --standalone 要占 80
certbot certonly --standalone -d audio.ceruleanavi.net
```

**挂整个 `/etc/letsencrypt`。** `live/` 下是软链，指向 `archive/`；只挂 `live` 或只挂一个 pem，续期后容器里仍是旧 inode。

续期不用重启。每次握手会 stat 证书，换了就重读；成功记 `tls certificate reloaded`（带 `not_after`），失败继续发旧的并记 `could not reload the TLS key pair`。想确认续期生效，看那一行的到期时间有没有往后挪——只在续期后第一次有人握手时出现。

走 Docker 不必 `chgrp ssl-cert`：容器以 root 读只读挂载。systemd + `DynamicUser` 那条路才会在续期当天撞上 0600 私钥。

### 2. 登录 GHCR、填环境、起容器

```bash
echo <PAT> | docker login ghcr.io -u <GitHub用户名> --password-stdin
# PAT 需要 read:packages
# 想免登录：GitHub → Packages → can-voice → 把包可见性改成 public
# （改的是包不是仓库；只能在 Web UI 改）

cd can-voice/server
cp .env.example .env
```

`.env` 必填：

| 变量 | 作用 |
|---|---|
| `CAN_VOICE_DOMAIN` | 拼 `/etc/letsencrypt/live/<域名>/`，和 certbot 申请时一字不差 |
| `CAN_VOICE_API_PUBKEY` | 上一步的公钥 |
| `ATIS_CID` / `ATIS_PASSWORD` | 机队用的**真实成员账号**。不填则容器能起、通播是哑的。空串也算没填好（#69） |

其余有默认值：`CAN_VOICE_PORT=64738`、`CAN_FSD_FEED`、机队连 `audio.ceruleanavi.net:64738`。机队**连外面的域名，不是 compose 内部主机名**——票按域名校证书，而且它连不上的时候成员也连不上。

```bash
docker compose up -d
docker compose logs -f
```

起来的样子：日志有监听地址，没有 `CAN_VOICE_* is required`。缺任何必需项直接退出，不会用默认值偷跑。

镜像是 distroless，**没有 shell**。看日志用 `docker compose logs`，不要 `docker exec`。

`docker compose ps` 里端口必须是 `0.0.0.0:64738->64738/udp`（或你改过的 `CAN_VOICE_PORT`）。写成 TCP，QUIC 一个包都进不来，症状是握手超时。

compose **只声明 `image`，没有 `build`**。加了 `build`，本地没镜像时会安静地从源码构建，和 CI 推上去的不是同一个。

### 3. 放行 UDP

```bash
ufw allow 64738/udp        # 或 firewall-cmd --add-port=64738/udp --permanent
```

云厂商安全组再放一层。漏了的症状同样是握手超时，和「网络封了 QUIC」长得一样。

测、暂不占 64738 时：`.env` 里 `CAN_VOICE_PORT=16438`（举例），防火墙放那个端口；客户端设置里把语音服务器改成 `audio.ceruleanavi.net:16438`。容器内仍听 `:64738`。

### 4. 从另一台机器验

不要在服务器本机验，那样没过防火墙。

```bash
# TOKEN 来自 can-api POST /api/v1/voice/token（成员 CAN 号 + 密码）
cargo run -p can-voice-client --example canvoice-cli -- \
  --server audio.ceruleanavi.net:64738 --token "$TOKEN"
```

没有「跳过证书」的开关。然后用 [`manual-test.md`](manual-test.md) 最短那 7 条：两份 `audio-for-can` 对讲。

安装包在 GitHub Release。`vXX.0.Z`（中间位是 0）是测试版。Windows 会弹 SmartScreen。没有 macOS 包。

## 日常

```bash
cd can-voice/server
docker compose pull
docker compose up -d
```

现在 tag 是 `latest`，回滚要自己记住 `sha-<commit>`（#78）。进程重启走关闭码 0，客户端会重连；已经连着的会话在证书热加载时不受影响。

客户端发新版是打 `v*` 标签（或 `gh workflow run release.yml -f version=v27.0.4`），和服务器镜像是两件事。

## 不用 Docker

```bash
go build -o can-voice ./server/cmd/can-voice
# 环境变量见 server/README.md；单元在 server/deploy/can-voice.service
```

这条路要自己处理 Let's Encrypt 私钥权限（`ssl-cert` 组 + deploy hook），续期当天文件权限会回到 `root:root 0600`。

机队是另一个二进制（`cargo build -p can-voice-atis --release`），仍要 `edge-tts` 和 `ffmpeg`。

## 以后切流（现在不做）

1. 手工测试清单过完，机队那几条已知停播 bug 已修或可接受。
2. 约定窗口，通知换客户端。
3. 停 Murmur（它占着 TCP+UDP 64738）。
4. `CAN_VOICE_PORT=64738`，防火墙已放行。
5. 老客户端连上去会握手失败——这是预期，不是回退信号。

没有共存期：两套不能同端口，主机名又写死在老客户端里。

## 起来了但连不上

对照 `server/README.md`《排障》。最常见的：

- `token signature does not verify`：公钥和 can-api 私钥不是一对，或拿反了。
- `token expired` 且人人如此：时钟，不是密钥。对一下 NTP。
- `rating is below the minimum`：未定级，不是故障。
- 证书 `no such file`：`CAN_VOICE_DOMAIN` 和 certbot 不一致，或没挂整个 letsencrypt。
- 外面超时、容器里正常：映射漏了 `/udp`，或安全组没放。
