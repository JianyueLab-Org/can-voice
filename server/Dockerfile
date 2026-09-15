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
# QUIC 走 UDP。**映射端口时 /udp 不能漏**——漏了的症状是握手超时，
# 和"这个网络封了 QUIC"长得一模一样。
EXPOSE 64738/udp
ENTRYPOINT ["/can-voice"]
