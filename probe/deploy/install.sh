#!/usr/bin/env bash
# 把探针服务端装到一台 Debian/Ubuntu 主机上。
#
# 用法：
#     probe/deploy/install.sh <ssh目标> [域名]
#     probe/deploy/install.sh root@1.2.3.4 probe.ceruleanavi.net
#
# 域名默认 probe.ceruleanavi.net，**跑之前 A 记录要已经指过来**——certbot
# 的 standalone 校验当场就要解析得到这台机器。
#
# 这个脚本只是把 README-部署.md 的一、二两节串起来，理由都写在那边。串成脚本
# 的理由只有一条：**域名要出现在五个地方**（证书申请、两条权限、单元文件里的
# 两条路径、以及最后的外网验证），手敲五遍里错一遍的症状是"握手超时"——
# 和"QUIC 在这个网络被封了"长得一模一样，而那正是本次实验要测的东西。
#
# 丢弃型代码：探针出了结论整个 probe/ 目录都要删，所以这里不做幂等、不做回滚。
set -euo pipefail
cd "$(dirname "$0")/../.."

host=${1:?用法: install.sh <ssh目标> [域名]}
domain=${2:-probe.ceruleanavi.net}
port=64739

say() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

say "1/6 本地构建 linux/amd64 服务端"
GOOS=linux GOARCH=amd64 CGO_ENABLED=0 \
  go build -trimpath -ldflags="-s -w" -o .temp/can-voice-probe-server ./probe/server
ls -lh .temp/can-voice-probe-server

say "2/6 生成单元文件（域名替换成 ${domain}）"
sed "s/probe\.ceruleanavi\.net/${domain}/g" probe/deploy/can-voice-probe.service \
  > .temp/can-voice-probe.service
grep -n "letsencrypt" .temp/can-voice-probe.service

say "3/6 传到 ${host}"
scp .temp/can-voice-probe-server "${host}:/usr/local/bin/can-voice-probe-server"
scp .temp/can-voice-probe.service "${host}:/etc/systemd/system/can-voice-probe.service"

say "4/6 证书与权限"
# certbot 的 standalone 要占 80 端口；被占住的话它报的是一句含糊的校验失败，
# 所以先自己看一眼。
ssh "$host" bash -s -- "$domain" <<'REMOTE'
set -euo pipefail
domain=$1
command -v certbot >/dev/null || { echo "这台机器上没有 certbot，先装：apt install certbot"; exit 1; }
if ss -lntp 2>/dev/null | grep -q ':80 '; then
  echo "80 端口被占着，certbot --standalone 会失败。先停掉占用它的服务："
  ss -lntp | grep ':80 '
  exit 1
fi
certbot certonly --standalone --non-interactive --agree-tos --register-unsafely-without-email -d "$domain"

# **私钥权限要单独处理，否则 Restart=always 会安静地反复重启。**
# Let's Encrypt 默认把 privkey.pem 放成 root:root 0600，而单元里的
# DynamicUser=yes 拿到的是一个动态 uid，读不到。
getent group ssl-cert >/dev/null || groupadd --system ssl-cert
chgrp ssl-cert "/etc/letsencrypt/live/${domain}/privkey.pem"
chmod 640      "/etc/letsencrypt/live/${domain}/privkey.pem"
# live/ 和 archive/ 是软链，两层目录也要能进去。
chmod 755 /etc/letsencrypt/live /etc/letsencrypt/archive
ls -l "/etc/letsencrypt/live/${domain}/privkey.pem"
REMOTE

say "5/6 放行 UDP ${port} 并启动"
# **这一条最容易漏，而漏了的症状不是"端口不通"而是握手超时**，
# 看起来和"QUIC 在这个网络被封了"一模一样。
ssh "$host" bash -s -- "$port" <<'REMOTE'
set -euo pipefail
port=$1
if command -v ufw >/dev/null && ufw status | grep -q "Status: active"; then
  ufw allow "${port}/udp"
elif command -v firewall-cmd >/dev/null && firewall-cmd --state >/dev/null 2>&1; then
  firewall-cmd --permanent --add-port="${port}/udp" && firewall-cmd --reload
else
  echo "没有检测到 ufw 或 firewalld。**如果云厂商那边还有安全组，去放行 UDP ${port}**"
  echo "——这一层脚本看不见，而漏了的症状和本次要测的现象一模一样。"
fi
systemctl daemon-reload
systemctl enable --now can-voice-probe
sleep 2
systemctl --no-pager --lines=20 status can-voice-probe || true
REMOTE

say "6/6 从这台机器验一次（不带 -insecure，顺便验证书链）"
# **报告不要落在 probe/reports/ 里。** 那个目录是收真实样本用的，而 analyse
# 一视同仁地把里面每一份都算成一个会话——一份自测报告（丢包 0%、就在机房隔壁）
# 混进去，就是往一个要支撑多年传输层决策的样本里掺了一份假数据。
probe_bin=probe/dist/can-voice-probe-$(go env GOOS)-$(go env GOARCH)
[ -x "$probe_bin" ] || { echo "没有 $probe_bin，先跑 probe/build.sh"; exit 1; }
mkdir -p .temp/selftest
echo "跑完会在 .temp/selftest 下留下一份 json。约两分钟。"
( cd .temp/selftest && exec "../../${probe_bin}" -server "${domain}:${port}" -carrier 自测 )

say "完成。接下来：probe/build.sh 发二进制，收够样本后 go run ./probe/client analyse probe/reports"
