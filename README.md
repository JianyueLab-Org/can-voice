# can-voice

Cerulean Aviation Network 的语音层：QUIC 语音服务端与客户端核心库。

设计文档见 can-audio 仓库的 `docs/superpowers/specs/2026-09-12-can-voice-design.md`。

```
server/                 Go：语音服务端（P2，Task 1–11 已落地）
crates/can-voice-proto  Rust：线协议。与 Go 侧共测 server/testdata/wire-golden.json
crates/can-voice-client Rust：客户端核心库（P3）
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
brew install autoconf automake libtool     # macOS
apt install autoconf automake libtool      # Debian/Ubuntu
```

缺了的话报的是一条看不出所以然的 `Failed to autogen Opus`。

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

## 手工验证

```bash
cargo run -p can-voice-client --example canvoice-cli -- \
  --server 127.0.0.1:64738 --token "$(cat target/e2e/token.txt)" \
  --root target/e2e/ca.der --rx 118000,121800
```
