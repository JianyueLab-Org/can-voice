# can-voice

Cerulean Aviation Network 的语音层：QUIC 语音服务端与客户端核心库。

设计文档见 can-audio 仓库的 `docs/superpowers/specs/2026-09-12-can-voice-design.md`。

```
server/                 Go：语音服务端（P2，Task 1–11 已落地）
crates/can-voice-proto  Rust：线协议。与 Go 侧共测 server/testdata/wire-golden.json
crates/can-voice-client Rust：客户端核心库（P3）
crates/can-voice-ptt    Rust：PTT（键盘 / 鼠标侧键 / 手柄），四个桌面端共用
crates/can-voice-atis   Rust：服务端通播机器人（P4 Task 8）
probe/                  P1 的一次性连通性探针，结论产出后删除
```

`probe/` 的代码已齐、**尚未部署**——剩下的部署、发放、收数与判定见
`probe/deploy/README-部署.md`，给测试用户的说明是 `probe/README-测试说明.md`。

## 服务端（Go）

```bash
go build ./... && go vet ./... && go test ./server/... -race
```

运行方式与全部环境变量见 `server/README.md`，那里也有线协议、关闭码与排障。

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
| `ATIS_CID` / `ATIS_PASSWORD` | 必需 | **一个真实成员账号。** 没有任何绕过账号的捷径，`no_shortcut_for_any_account` 钉着 |
| `CAN_API_ORIGIN` | `https://api.ceruleanavi.net` | 换票的地方（`POST /api/v1/voice/token`） |
| `CAN_VOICE_SERVER` | `audio.ceruleanavi.net:64738` | 语音服务端 |
| `CAN_FSD_DATAFEED` | `https://data.ceruleanavi.net/v1/data.json` | 席位从哪来 |
| `ATIS_TTS_ARGV` | `edge-tts --voice {voice} --text {text} --write-media {out}` | 合成命令模板 |
| `ATIS_VOICE_EN` / `ATIS_VOICE_ZH` | `en-US-AriaNeural` / `zh-CN-XiaoxiaoNeural` | 两种语言的嗓子 |
| `ATIS_POLL_SECS` | `30` | 多久看一次 datafeed |

三条和四个桌面客户端**相反**的规矩，都写在代码里：

- **不设有界重连。** 桌面端掉线三次就下线；一支给三次机会就放弃的机队，会在一次
  网络抖动之后让全网 ATIS 悄无声息地下线，而没有任何人在看着它。
- **死掉的那一路要重新拉起。** 判据是"那个任务还活着吗"，不是"在不在表里"——
  只查在不在表里的话，一次瞬时故障就让这个席位永远停播，而管理器还以为它好好的、
  每 30 秒给它更新一次文本。
- **取不到 datafeed 不停播。** 正在播的照常，报文停在最后一次取到的那份。
