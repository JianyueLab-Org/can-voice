# syntax=docker/dockerfile:1
# ↑ 这一行**必须是文件的第一行**。解析指令出现在任何注释之后就只是一条普通
#   注释，而下面那个健康探针的源码是用 heredoc（`COPY <<EOF`）直接写进构建阶段
#   的——那是 dockerfile 1.4 起才有的语法，旧前端会把它当成一个名叫 `<<'EOF'`
#   的文件去找，报的是 "file not found"。

# can-voice 语音服务端的镜像。
#
# 构建上下文是仓库根（go.mod 在那儿）：
#     docker build -f server/Dockerfile -t can-voice .

# 交叉编译而不是靠 QEMU 跑多架构：服务端是纯 Go、CGO_ENABLED=0，换个 GOARCH
# 就是换个环境变量的事，而 QEMU 模拟一次 arm64 构建要慢十几倍。
FROM --platform=$BUILDPLATFORM golang:1.27-alpine AS build
WORKDIR /src
# 先只拷依赖清单，让 go mod download 那一层能被缓存住。
COPY go.mod go.sum ./
RUN go mod download
COPY server/ ./server/
ARG TARGETOS
ARG TARGETARCH
RUN CGO_ENABLED=0 GOOS=${TARGETOS} GOARCH=${TARGETARCH} \
    go build -trimpath -ldflags="-s -w" -o /out/can-voice ./server/cmd/can-voice

# 健康探针，和服务端一起交叉编译进同一个 /out。
#
# **为什么是第二个二进制，而不是 HEALTHCHECK 里的一行 shell。** 下面那层是
# distroless static——没有 shell、没有 curl、没有 nc，HEALTHCHECK 能执行的只有
# 镜像里已有的可执行文件；而 `/can-voice` 自己没有"检查一下就退出"的模式：
# 给它一份配置它就去监听，而那个口正被它自己占着。
#
# **判据是 64738/udp 绑不上。** 绑得上说明没有人在听——这正是部署那边唯一在乎
# 的事：QUIC 的监听没起来，在客户端那边和"这个网络封了 QUIC"长得一模一样，而
# 容器还好端端地活着。只认 EADDRINUSE 一种错：换成"随便什么错都算健康"，
# 一个拼错的 CAN_VOICE_ADDR 会让探针永远报绿。
#
# 源码写在这里而不是 server/cmd/ 下面：它服务的是这个镜像，不是服务端的代码，
# 也不该进 gofmt / go vet 那几个 job 的视野。
COPY <<'EOF' /src/healthcheck/main.go
package main

import (
	"errors"
	"fmt"
	"net"
	"os"
	"syscall"
)

func main() {
	addr := os.Getenv("CAN_VOICE_ADDR")
	if addr == "" {
		addr = ":64738"
	}
	c, err := net.ListenPacket("udp", addr)
	if errors.Is(err, syscall.EADDRINUSE) {
		return // 有人占着这个口，也就是服务端的监听还在
	}
	if err != nil {
		fmt.Fprintf(os.Stderr, "healthcheck: cannot probe %s: %v\n", addr, err)
		os.Exit(1)
	}
	c.Close()
	fmt.Fprintf(os.Stderr, "healthcheck: nothing is listening on %s\n", addr)
	os.Exit(1)
}
EOF
RUN CGO_ENABLED=0 GOOS=${TARGETOS} GOARCH=${TARGETARCH} \
    go build -trimpath -ldflags="-s -w" -o /out/healthcheck ./healthcheck

# distroless static：没有 shell、没有包管理器，只有这一个静态二进制。
#
# **用 root 那个 tag 而不是 :nonroot，这是有意的。** Let's Encrypt 默认把
# privkey.pem 放成 root:root 0600。宿主机上跑 systemd 那条路要靠 `chgrp ssl-cert`
# + `chmod 640` 才能让 DynamicUser 读到，而那一步**漏了的症状是服务安静地反复
# 重启**，而且不会在测试里出现、只会在**续期那天**出现——证书换了新文件，权限
# 回到默认值（README 的《排障》里写着这一条）。容器里以 root 读一个只读挂载的
# 证书目录，整个坑不存在。
FROM gcr.io/distroless/static-debian12:latest
COPY --from=build /out/can-voice /can-voice
COPY --from=build /out/healthcheck /healthcheck
# QUIC 走 UDP。**映射端口时 /udp 不能漏**——漏了的症状是握手超时，
# 和"这个网络封了 QUIC"长得一模一样。
EXPOSE 64738/udp

# **exec 形式，不能写成 `HEALTHCHECK CMD /healthcheck`。** shell 形式会被包成
# `/bin/sh -c …`，而这个镜像里没有 /bin/sh——症状是健康状态永远是 unhealthy，
# 而日志里只有一句 "exec: /bin/sh: no such file or directory"。
#
# start-period 给得短：进程起来就绑口，没有预热。
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/healthcheck"]

ENTRYPOINT ["/can-voice"]
