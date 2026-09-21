# can-voice

Cerulean Aviation Network 的语音层：QUIC 语音服务端与客户端核心库。

设计文档见 can-audio 仓库的 `docs/superpowers/specs/2026-09-12-can-voice-design.md`。

```
server/                 Go：语音服务端（P2，Task 1–11 已落地）
crates/can-voice-proto  Rust：线协议。与 Go 侧共测 server/testdata/wire-golden.json
crates/can-voice-client Rust：客户端核心库（P3）
crates/can-voice-ptt    Rust：PTT（键盘 / 鼠标侧键 / 手柄），四个桌面端共用
crates/can-voice-token  Rust：拿凭据换短期票，四个桌面端共用
crates/can-voice-app    Rust：四个桌面端共用的快照 / 会话监管 / 命令面
crates/can-voice-settings Rust：设置读写。单独一个 crate，见下
crates/can-voice-update Rust：更新检查（问 can-api，只报告不动手）。同上
crates/can-voice-log    Rust：日志落盘（4 份 × 1 MiB）、panic 钩子、回传。同上
crates/can-voice-fsd    Rust：FSD 协议客户端那一侧，通播端与飞行员端共用
crates/can-voice-sim    Rust：两个飞行员客户端共用——模拟器链路、他机表、
                        在线席位表、文字消息记录
crates/can-voice-atis   Rust：通播的逻辑（报文 / 模板 / 读法）＋服务端通播机器人
apps/controller         Tauri：管制语音客户端 audio-for-can（P4 Task 4）
apps/atis               Tauri：通播制作客户端 atis-for-can（P4 Task 5）
apps/xpc / apps/msfs    Tauri：两个飞行员客户端 xpc-for-can / msfs-for-can
```

`can-voice-settings` 和 `can-voice-update` 各自一个 crate 而不是
`can-voice-app` 里的两个模块，是同一条理由：**通播制作客户端不该为了读一个 JSON
文件、或者查一次更新，把 `can-voice-ptt` 拉进来**——那条路上挂着 rdev/gilrs，
Linux 上要 libx11 和 libudev，而它连 PTT 都没有。

**桌面端不在 workspace 里**（根 `Cargo.toml` 的 `exclude = ["apps"]`）：它们各自
拖着 wry/webkit 一整棵树，进来的话 `cargo test --workspace` 每次都要构建一个 GUI
工具链，而核心库那几个 crate 的测试本来是秒级的。各自在 `apps/<name>/src-tauri`
里构建。

dev 端口接在网站那排 432x 后面：4330 controller、4331 atis，4332 / 4333 留给
xpc / msfs。

**平台**：Windows 和 Linux 都出包（`.exe`/`.msi` 和 `.deb`/`.AppImage`），
macOS 还没有——缺的是 Developer ID 证书和公证那一套。can-audio 现在发的四个
客户端**只有 Windows**，所以 Linux 这一份是新能力而不是回归：X-Plane 本来就有
Linux 版，而管制端和通播端跟模拟器无关。

**两个平台都不签名，这是 2026-09-17 定下的，不是漏了。** Windows 签名要么开
Azure Artifact Signing 并通过身份验证，要么买一张放在云 HSM 里的 OV 证书，
两条路都还没走；在那之前 Windows 会弹 SmartScreen 的"已保护你的电脑"，
release 正文里写明了怎么点过去（`.github/release-notes/common.md`）。
接上签名时，那一段要跟着删掉。

**Linux 上的键盘 PTT 只在 X11 下有效。** 全局按键监听走 Xlib，而 Wayland 不允许
一个普通程序监听全局按键。手柄 PTT 和界面上那个「按住发话」按钮不受影响。

## 通播由服务端机队出声，桌面那支只做稿子

`atis-for-can` 挂 FSD 席位、发文字、答查询，**不出声**；声音归
`crates/can-voice-atis` 的可执行文件那一半，部署上是
`server/docker-compose.yml` 里的第二个容器。

这条线要划清楚，因为它一旦模糊，同一个频率上就会有两个声音——can-audio 那边
就是这样（桌面那支开自己的 Mumble 连接，服务端机队又把 datafeed 里的全播一遍），
而它的文档从没提过。

选机队的理由是它**不会睡觉**。代价是它手上只有 `text_atis`，念的是电码原文
（`09004MPS` → "zero niner zero zero four MPS"）而不是模板渲染出来的语音形态。
详见 `crates/can-voice-atis/src/lib.rs` 的模块头。

## 怎么从 Mumble 切过来

**已经拍板：一并替换，没有共存期。** 三条结论：

- **没有"改一行数据库就把所有人挪过去"这条路。** Mumble 的主机名写死在老客户端
  里，只有 xpc / msfs 把它当可改的默认值。换的单位是"人换客户端"。
- **两套不能跑在同一台机器的同一个端口上**（Murmur 占着 TCP+UDP 64738，
  can-voice 的 QUIC 要 UDP 64738）——在一并替换这条路上这是**帮手**：can-voice
  接管同一个地址，老客户端握手就失败。而分阶段方案里最难被报告的故障，正是
  掉队的人待在一个正常但空无一人的频率里。
- **通播由服务端机队出声**，见上一节。它是 compose 里的第二个容器，拿一个真实
  成员账号换票，对语音服务端而言就是个普通客户端。

## 传输只有 datagram，没有 stream 回退

P2 Task 12（stream 回退通道）**定为不做**，P1 的连通性探针也随之删掉：
网络可达性不在这个项目的考虑范围内。语音走 QUIC 不可靠 datagram 一条路，
UDP 不通就是不通，客户端不会退到别的通道上去。

## 服务端（Go）

```bash
go build ./... && go vet ./... && go test ./server/... -race
```

部署清单见 [`docs/deploy.md`](docs/deploy.md)。变量、线协议、关闭码与排障仍在 `server/README.md`。

## 客户端核心库（Rust）

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

**构建前置：autotools。** `audiopus_sys` 从源码构建 libopus 并**静态链接**进二进制
（`audiopus_sys` 的 `static` feature），而它的 build.rs 走的是
`sh autogen.sh && sh configure && make`：

```bash
brew install autoconf automake libtool                      # macOS
apt install autoconf automake libtool libasound2-dev        # Debian/Ubuntu
```

缺了的话报的是一条看不出所以然的 `Failed to autogen Opus`。

**Linux 还要 `libx11-dev libxi-dev libxtst-dev libudev-dev`**（`can-voice-ptt` 的
rdev 走 Xlib、gilrs 走 udev），以及 `libasound2-dev`：cpal 在那边走 ALSA，缺了报的是
`failed to run custom build command for \`alsa-sys\``。macOS 上 cpal 走 CoreAudio，
所以这一条**在 mac 开发机上永远不会露头**——CI 的 Linux runner 第一次跑就抓到了它。

**静态链接不是可选的，也不要"先用系统的 libopus 顶一下"。** 它消灭的是 Python 版
那一整类故障：`opus.dll` 没跟着打包，程序照常启动，语音静默失效。验证方法：

```bash
cargo build -p can-voice-client --release
otool -L target/release/deps/can_voice_client-*   # macOS：不该出现任何 opus
ldd    target/release/deps/can_voice_client-*     # Linux：同上
```

MSRV 是 1.80（`Cargo.toml` 的 `rust-version`）。clippy 的 `incompatible_msrv`
会把越界的 API 当错误咬住，所以那个数字是承重的，不是装饰。

## 端到端测试

唯一能发现各层拼接错误的测试：它起一个**真的**服务端，跑握手、订阅、拒绝。
夹具一条命令产出（自签的两级证书链、Ed25519 密钥对、一张有效 token 和一张过期的）：

```bash
mkdir -p target/e2e
go build -o target/e2e/can-voice ./server/cmd/can-voice
go run ./server/cmd/can-voice-e2e-fixture
CAN_VOICE_E2E=1 cargo test -p can-voice-client --test e2e
```

不设 `CAN_VOICE_E2E` 时这几条测试自己跳过——没有夹具的机器不该因为它们红。

**客户端没有"跳过证书校验"的开关，而且不会有。** 自签证书是通过
`Config::extra_roots`（额外的**根**证书）进来的，链校验一步不少。一个 `insecure`
标志一旦存在就会有人在生产里打开它，而这条链路上跑的是成员的网络密码。

端到端里最有分量的一条是 `audio_crosses_the_wire_from_one_client_to_another`：
两个客户端、两个账号（服务端对同一个 CID 会**顶号**），音频从一个穿到另一个。
它一条就走通了成帧 → Opus 编码 → 序号 → 数据报 → 服务端扇出 → 抖动缓冲 → 解码 →
混音 → 发言记账；其余几条只验到控制面为止。

## 手工验证

真声卡、真票、真顶号、真通播机队那一套，清单在 [`docs/manual-test.md`](docs/manual-test.md)。下面两条命令只开核心库的命令行客户端，用来确认本机夹具还活着。

```bash
# 只收听（--audio 才真的开声卡）
cargo run -p can-voice-client --example canvoice-cli -- \
  --server 127.0.0.1:64738 --token "$(cat target/e2e/token.txt)" \
  --root target/e2e/ca.der --audio --rx 118000,121800

# 收 + 发
cargo run -p can-voice-client --example canvoice-cli -- \
  --server 127.0.0.1:64738 --token "$(cat target/e2e/token-b.txt)" \
  --root target/e2e/ca.der --audio --rx 121800 --tx 121800
```

**夹具里的 token 有效期 5 分钟**（`auth.maxTokenLifetime` 是 10 分钟，短有效期是
这套设计里唯一的吊销机制）。隔一会儿再跑要重新 `go run ./server/cmd/can-voice-e2e-fixture`
——端到端测试会认出这个情况并直接告诉你该跑哪条命令。

## 服务端通播机器人

没有界面、没有声卡：它盯着 can-fsd 的 datafeed，为每一个 `_ATIS` 席位起一路，
音频由 TTS 合成后经 `push_audio` 注入——走的是和麦克风**完全相同**的那条路
（成帧、序号、首尾帧、扇出）。

```bash
cargo run -p can-voice-atis
```

外部依赖两个，都要在 PATH 上：**TTS 命令**（默认 `edge-tts`）和 **ffmpeg**
（把合成出来的 mp3 转成 48 kHz 单声道 PCM）。Python 版的部署本来就要求 ffmpeg。

| 变量 | 默认 | 说明 |
|---|---|---|
| `ATIS_CID` / `ATIS_PASSWORD` | 必需 | **一个真实成员账号，rating ≥ 1。** 没有任何绕过账号的捷径，`no_shortcut_for_any_account` 钉着。未定级的账号换不到票：can-api 回 403 `insufficient_rating`，日志里说的是 rating，不是密码。**空串等于没填**——`docker-compose.yml` 注入的就是空串 |
| `CAN_API_ORIGIN` | `https://api.ceruleanavi.net` | 换票的地方（`POST /api/v1/voice/token`） |
| `CAN_VOICE_SERVER` | `audio.ceruleanavi.net:64738` | 语音服务端 |
| `CAN_FSD_DATAFEED` | `https://data.ceruleanavi.net/v1/data.json` | 席位从哪来 |
| `ATIS_TTS_ARGV` | `edge-tts --voice {voice} --text {text} --write-media {out}` | 合成命令模板 |
| `ATIS_VOICE_EN` / `ATIS_VOICE_ZH` | `en-US-ChristopherNeural` / `zh-CN-YunxiNeural` | 两种语言的嗓子，见下 |
| `ATIS_POLL_SECS` | `30` | 多久看一次 datafeed。**下限 5 秒**；写坏或者更小就起不来，不悄悄回退 |

四条和四个桌面客户端**相反**的规矩，都写在代码里：

- **不设有界重连。** 桌面端掉线三次就下线；一支给三次机会就放弃的机队，会在一次
  网络抖动之后让全网 ATIS 悄无声息地下线，而没有任何人在看着它。
- **死掉的那一路要重新拉起。** 判据是"那个任务还活着吗"，不是"在不在表里"——
  只查在不在表里的话，一次瞬时故障就让这个席位永远停播，而管理器还以为它好好的、
  每 30 秒给它更新一次文本。
- **取不到 datafeed 不停播。** 正在播的照常，报文停在最后一次取到的那份。
  **一份取到了但缺 `atis` 字段的文档同样不停播**：它比一次网络错误更不可能
  说明"全网的 ATIS 都下线了"。只有字段在、而且是个空数组才是"没人在播"。
- **被顶号（关闭码 2）退到最大退避，而不是彻底停下。** 桌面端遵守的是"停止
  重连、告诉用户账号在别处登录了"；机队没有用户可以告诉，真停下就没有人把这个
  席位拉回来——顶掉它的那一套可能几分钟后自己就下线了。所以同一个 `ATIS_CID`
  跑了两套机队时（比如演练环境连到了生产服务端），两边不再按席位每秒互踢一轮，
  而是各自等一分钟再试一次，日志里单独一条 warn 说明账号在别处被用着。

播出的两件事**照旧和 can-audio 一样**，是拍过板的保留，不是没来得及改：

- **嗓子是那两个男声**：`zh-CN-YunxiNeural` / `en-US-ChristopherNeural`
  （`can-audio/server/ATIS/mumble.py:317,324`）。换一种声音全网都听得出来，
  而"今天的通播听着不对"是一条没人报得上来的故障。要换改 `ATIS_VOICE_*`。
- **先播中文、再播英文**（`mumble.py:371-388`）。稿子里英文在前那是分隔符的
  约定（`en|zh`），不是播出的顺序。

嗓子按文本里**有没有汉字**选，与分隔符无关（旧版的判据，`mumble.py:307`）：
机队播的是 datafeed 里所有 `_ATIS` 席位，而别的来源发的纯中文通播没有 `|`。
