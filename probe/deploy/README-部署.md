# P1 探针部署与收数

P1 的**代码已经齐了**——服务端、客户端两轮测量、报告输出、判定 `Verdict`，测试全绿。
剩下的全是运维动作，一件都不在仓库里：**部署一台服务器、把二进制发出去、收够样本、
写结论文档**。这份文件是那几步的清单。

结论文档是 `docs/p1-connectivity-findings.md`（**不在 `probe/` 下，所以不随探针删除**）。
它产出之前，下游两件事一直停着：

- **P2 Task 12（stream 回退通道）做不做**，完全由这份结论决定；
- **P3 不得出现 `Hello.transport` 字段**（P3 修订件 M1）——一个宣称了未构建行为的字段
  比没有这个字段更糟。

结论产出、P2 Task 12 定夺之后，**整个 `probe/` 目录删除**。它是丢弃型代码。

---

## 一、服务端

### 1. 域名

给 `probe.ceruleanavi.net` 加一条 A 记录指向探针主机。**不要复用 `audio.ceruleanavi.net`**：
那是现网 Mumble 的地址，探针要独立的证书和独立的 UDP 端口，混在一起的唯一结果是
哪天想关探针却不敢动。

### 2. 证书

```bash
certbot certonly --standalone -d probe.ceruleanavi.net
```

**私钥的权限要单独处理，否则 `Restart=always` 会安静地反复重启。** Let's Encrypt 在 Debian 上
默认把 `live/*/privkey.pem` 放成 `root:root 0600`，而单元里的 `DynamicUser=yes` 拿到的是一个
动态 uid，读不到：

```bash
chgrp ssl-cert /etc/letsencrypt/live/probe.ceruleanavi.net/privkey.pem
chmod 640      /etc/letsencrypt/live/probe.ceruleanavi.net/privkey.pem
```

`can-voice.service` 有一模一样的坑，`server/README.md` 的《排障》一节写着同一条。
探针是一次性的，活不到证书续期那天，所以 deploy hook 可以省；**生产服务端不能省**。

### 3. 起服务（Docker，推荐）

```bash
docker run -d --name can-voice-probe --restart always \
  -p 64739:64739/udp \
  -v /etc/letsencrypt:/etc/letsencrypt:ro \
  ghcr.io/jianyuelab-org/can-voice-probe:latest \
  -addr :64739 \
  -cert /etc/letsencrypt/live/probe.ceruleanavi.net/fullchain.pem \
  -key  /etc/letsencrypt/live/probe.ceruleanavi.net/privkey.pem
```

**`/udp` 不能漏。** 漏了的症状是握手超时，和"QUIC 在这个网络被封了"长得一模一样，
而那正是本次实验要测的东西。

**走 Docker 就不用做上面第 2 节末尾那两条 `chgrp`/`chmod`。** 容器里以 root 读一个
只读挂载的证书目录，`DynamicUser` 读不到 0600 私钥那个坑整个不存在——那是这条路
相对 systemd 的实际好处，不是省事。

镜像里**没有 shell**（distroless static），看日志用 `docker logs -f can-voice-probe`，
不要指望 `docker exec` 进去。

镜像由 `.github/workflows/probe-image.yml` 在 main 上构建并推到 GHCR，
`latest` 和一个 `sha-<commit>` 两个标签，`linux/amd64` + `linux/arm64`。

> **仓库是私有的，所以这个包默认也是私有的。** 拉之前要先
> `echo <PAT> | docker login ghcr.io -u <你的用户名> --password-stdin`
> （PAT 需要 `read:packages`）。想免登录拉取，就去
> `github.com/orgs/JianyueLab-Org/packages` 把 `can-voice-probe`
> 这个包的可见性改成 public —— **改的是包，不是仓库**，仓库可以继续私有。

### 3'. 二进制与单元（不想用 Docker 时）

**二到五这几步**串成了一个脚本（第一步的 A 记录要先加好，脚本没法替你做——
certbot 的 standalone 校验当场就要解析得到这台机器）：

```bash
probe/deploy/install.sh root@<host> probe.ceruleanavi.net
```

串成脚本的理由只有一条：域名要出现在五个地方（证书申请、两条权限、单元文件里的两条
路径、最后的外网验证），手敲五遍里错一遍的症状是"握手超时"——和"QUIC 在这个网络被封
了"长得一模一样，而那正是本次实验要测的东西。脚本做的事和下面这几段一字不差，
想一步步来就照着下面手动执行。

```bash
GOOS=linux GOARCH=amd64 CGO_ENABLED=0 go build -trimpath -o can-voice-probe-server ./probe/server
scp can-voice-probe-server            <host>:/usr/local/bin/
scp probe/deploy/can-voice-probe.service <host>:/etc/systemd/system/
ssh <host> 'systemctl daemon-reload && systemctl enable --now can-voice-probe'
```

单元里**没有** `AmbientCapabilities=CAP_NET_BIND_SERVICE`，这是对的而不是漏了：
监听的是 64739，1024 以上的端口不需要那个能力。

### 4. 防火墙

放行 **UDP 64739**。这一条最容易漏，因为漏了的症状不是"端口不通"而是**握手超时**，
看起来和"QUIC 在这个网络被封了"一模一样——而那正是本次实验要测的东西。
先自己从外网验一次（下一步），再发给任何人。

**云厂商的安全组是第二层，脚本看不见它。** `install.sh` 只认得机器上的 ufw 和
firewalld；阿里云/腾讯云/AWS 控制台里那一层要自己去放行 UDP 64739，而漏了它的症状
和上面一模一样。

### 5. 从外网验证一次

在**不是服务器本机**的机器上：

```bash
./probe/dist/can-voice-probe-darwin-arm64 -carrier 自测
```

`-server` 的默认值就是 `probe.ceruleanavi.net:64739`，不用带。
**不要带 `-insecure`**——这一步同时验的是 Let's Encrypt 证书链，加上那个开关就什么都没验。

应当看到：握手成功、两轮丢包率都很低、当前目录下生成一份
`can-voice-probe-<日期>-<时间>.json`。

---

## 二、发给测试用户

```bash
./probe/build.sh        # 产出 probe/dist/ 下四个二进制，各约 8–12 MB
```

`probe/dist/` 是 gitignored 的，每次现构建，不要提交。连同
`probe/README-测试说明.md` 一起发——那份是写给非技术用户的，四个平台怎么下、
怎么在终端里跑、macOS 的"无法验证开发者"怎么点，都在里面。

**跑一次约两分钟**（两轮各 60 秒 + 轮内 drain + 握手，约 2 分 5 秒）。

### 样本覆盖要求

| 要求 | 数量 | 谁在管 |
|---|---|---|
| 会话总数 | ≥ 8 | **代码里的 `minSessions`** |
| 不同的网络（`-carrier` 值） | ≥ 3 | **代码里的 `minCarriers`** |
| 大陆三家运营商（移动/联通/电信） | 各 ≥ 1 | 招募的时候自己盯 |
| 校园网或企业网 | ≥ 1 | 同上 |
| 操作系统 | ≥ 2 种 | 同上 |

### 招募文案

可以直接贴的一段（Discord/群），和 `README-测试说明.md` 一起发：

> **帮个忙：两分钟的语音连通性测试**
>
> 我们在给 CAN 做新的语音系统，用的传输方式（QUIC / UDP）和现在的不一样，所以要先确认
> 它在大家的网络里通不通。想请你跑一个小工具，**大约两分钟**，跑完会生成一个文本文件，
> 发回给我就行。
>
> 它**不上传任何东西**，文件里没有 IP、没有用户名、没有任何个人信息，你可以先打开看一遍
> 再决定发不发。
>
> 特别需要这几类网络的样本：**中国移动 / 中国联通 / 中国电信各来几份**，还有**校园网**和
> **公司网络**。如果你能在不同网络下各跑一次（家里、公司、手机热点），帮助最大。
>
> **连不上也请把文件发回来**——连不上本身就是我们最想知道的结果之一。
>
> 下载和操作说明见附件。

招募时要说清楚的三件事，缺一件就会掉样本：**两分钟**（不说时长没人点开）、
**不上传**（这是让人肯跑一个没签名的二进制的前提）、**失败也要发**
（不说这一句，失败的人会默默关掉窗口，而那恰好是最有价值的那份数据）。

### 收样本的过程中随时看构成

```bash
go run ./probe/client analyse probe/reports
```

样本不够时它照样会先印一段 `coverage`：

```
coverage (for recruiting — compare against the table in README-部署.md):
  - 5 session(s) across 3 network(s): 中国电信 ×3、中国移动 ×1、校园网 ×1
  - operating systems: windows ×3、darwin ×1、linux ×1
```

"还差 3 份"和"还差 3 份、而且联通一个都没有"是完全不同的两件事，而下面那张表的后三行
代码管不了。构成是**给招募看的，不驱动判定**，所以它和 `diagnostics` 分开印。

前两行是**硬的**：`Verdict` 在样本不足时拒绝出结论，而不是给一个看起来有依据的答案。
用 6 个人的数据决定一个要维护多年的传输层，比没有数据更危险。

`analyse.go` 还会在**排除掉不可用报告之后再检查一次**这两道门槛——否则一个
"12 份报告里 6 份跑残了"的样本会绕过它本身要防的情况。所以收到 8 份不等于够了，
以 `analyse` 的输出为准。

后三行代码管不了，只能在招募的时候盯：**一个只有电信用户的样本，测不出联通的中间设备
怎么对待 UDP**，而那正是这次要回答的问题。

---

## 三、收数与判定

把收回来的 JSON 全部放进 `probe/reports/`（也是 gitignored 的），然后：

```bash
go run ./probe/client analyse probe/reports
```

输出分四块，**它们的份量不一样，别混着读**：

- `coverage` —— 样本构成，**给招募看的，不驱动判定**；
- `diagnostics` —— 样本量与排除情况，**信息性的，不驱动判定**；
- `thresholds crossed` —— 真正越线、驱动 `NeedFallback` 的理由；
- `SEPARATE FINDINGS` —— 越线了但**不构成回退通道的理由**的发现。
  stream 回退活在同一条 QUIC 连接里，救不了这两类情况（整条连接被封、或者
  握手就过不去），它们要另想办法，不要拿来论证 Task 12。

最后一行是三选一：

```
VERDICT: inconclusive — collect more data
VERDICT: a stream fallback channel is required
VERDICT: datagram-only is sufficient
```

### 判定标准是写死的，不许事后调整

阈值在 `probe/client/analyse.go` 的常量里，**写在看数据之前**。数据回来之后觉得
"这条线画得不合适"，那是在为已知的答案挑一条能通过的标准，整个实验就白做了。
真要改，先说清楚为什么改、并且把改动记进结论文档。

---

## 四、写结论文档

`docs/p1-connectivity-findings.md`，至少要有：

1. **判定结果**，以及 `analyse` 的原始输出；
2. **样本构成**：几个会话、哪些网络、哪些操作系统；
3. **越线的那几条**和它们的实测值；
4. **那条必须写进去的限制**——探针两侧都是 Go/quic-go，而生产客户端是 Rust/quinn。
   本次测的是**网络**（UDP 能否通、能否持续），**不是库**。quinn 与 quic-go 在中间设备
   指纹、初始包大小、GSO 行为上的差异**不在本次测量范围内**，那要在封闭测试里用真实
   客户端复核。不写这一条，下一个人会拿这份结论去证明一件它没测过的事。

然后：定夺 P2 Task 12，删掉 `probe/`。
