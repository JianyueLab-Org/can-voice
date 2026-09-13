#!/usr/bin/env bash
# 构建探针客户端的分发二进制。丢弃型工具，不签名、不公证 ——
# 测试用户会被告知 macOS 需要右键打开，见 README-测试说明.md。
set -euo pipefail
cd "$(dirname "$0")/.."

out=probe/dist
rm -rf "$out"
mkdir -p "$out"

build() {
  local goos=$1 goarch=$2 ext=${3:-}
  echo "building ${goos}/${goarch}"
  GOOS=$goos GOARCH=$goarch CGO_ENABLED=0 \
    go build -trimpath -ldflags="-s -w" \
    -o "${out}/can-voice-probe-${goos}-${goarch}${ext}" ./probe/client
}

build windows amd64 .exe
build darwin  arm64
build darwin  amd64
build linux   amd64

ls -lh "$out"
