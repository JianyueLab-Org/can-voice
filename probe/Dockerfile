# 探针服务端的镜像。**丢弃型**：探针出了结论，整个 probe/ 目录连同这个文件一起删。
#
# 构建上下文是仓库根（go.mod 在那儿）：
#     docker build -f probe/Dockerfile -t can-voice-probe .

# 交叉编译而不是靠 QEMU 跑多架构构建：服务端是纯 Go、CGO_ENABLED=0，
# 换个 GOARCH 就是换个环境变量的事，而 QEMU 模拟一次 arm64 构建要慢十几倍。
FROM --platform=$BUILDPLATFORM golang:1.27-alpine AS build
WORKDIR /src
# 先只拷依赖清单，让 go mod download 那一层能被缓存住。
COPY go.mod go.sum ./
RUN go mod download
COPY probe/ ./probe/
ARG TARGETOS
ARG TARGETARCH
RUN CGO_ENABLED=0 GOOS=${TARGETOS} GOARCH=${TARGETARCH} \
    go build -trimpath -ldflags="-s -w" -o /out/can-voice-probe-server ./probe/server

# distroless static：没有 shell、没有包管理器，只有这一个静态二进制。
#
# **用 root 那个 tag 而不是 :nonroot，这是有意的。** Let's Encrypt 默认把
# privkey.pem 放成 root:root 0600，宿主机上跑 systemd 那条路要靠 `chgrp ssl-cert`
# + `chmod 640` 才能让 DynamicUser 读到——而那一步漏了的症状是服务安静地反复重启，
# README-部署.md 专门写了这个坑。容器里以 root 读一个 **只读挂载** 的证书目录，
# 整个坑就不存在了，这正是 docker 这条路相对 systemd 的实际好处。
FROM gcr.io/distroless/static-debian12:latest
COPY --from=build /out/can-voice-probe-server /can-voice-probe-server
# QUIC 走 UDP。**映射端口时 /udp 不能漏**——漏了的症状是握手超时，
# 和"QUIC 在这个网络被封了"长得一模一样，而那正是本次实验要测的东西。
EXPOSE 64739/udp
ENTRYPOINT ["/can-voice-probe-server"]
