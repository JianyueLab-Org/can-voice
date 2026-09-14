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

### 3. 二进制与单元

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

输出分三块，**它们的份量不一样，别混着读**：

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
