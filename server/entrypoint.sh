#!/usr/bin/env bash
# 统一 Rust 后端镜像的入口。
#
# 默认（无参数）：同时拉起两个进程
#   harness  --listen 0.0.0.0:43301 --resources /data/resources   # 诊断后端（对外 43301）
#   rrserver server --listen 0.0.0.0:43302 --config /etc/rrserver.toml  # 云端中继（43302）
#
# 传参时按参数执行，用于只跑一个进程 / 其他子命令：
#   docker run tcmi_server:local rrserver client --server wss://... --name home --token ...
#   docker run tcmi_server:local rrserver llm-server --config /etc/llm_server.toml
#   docker run tcmi_server:local harness --help
#   （首个参数以 - 开头时按 harness 处理，兼容 `docker run <img> --listen ...`）
#
# 就绪判定：任一进程退出 → 容器整体退出，由 restart 策略把两个进程一起拉起，
# 避免「容器还在跑、其中一个进程已经死了」这种静默半死状态。
set -uo pipefail

HARNESS_LISTEN="${HARNESS_LISTEN:-0.0.0.0:43301}"
HARNESS_RESOURCES="${HARNESS_RESOURCES_DIR:-/data/resources}"
RR_LISTEN="${RR_LISTEN:-0.0.0.0:43302}"
RR_CONFIG="${RR_CONFIG:-/etc/rrserver.toml}"

# 显式传参 → 直接执行
if [ "$#" -gt 0 ]; then
  case "$1" in
    harness|rrserver) exec "$@" ;;
    *) exec harness "$@" ;;
  esac
fi

echo "[entrypoint] harness  : $HARNESS_LISTEN (resources=$HARNESS_RESOURCES)"
echo "[entrypoint] rrserver : $RR_LISTEN (config=$RR_CONFIG)"

harness --listen "$HARNESS_LISTEN" --resources "$HARNESS_RESOURCES" &
harness_pid=$!
rrserver server --listen "$RR_LISTEN" --config "$RR_CONFIG" &
rrserver_pid=$!

# docker stop 时把信号转给两个子进程，否则容器会等到超时才被 kill
trap 'kill "$harness_pid" "$rrserver_pid" 2>/dev/null' TERM INT

# wait -n：任一子进程退出即返回
wait -n
code=$?
echo "[entrypoint] 子进程退出（status=$code），停止另一进程并退出容器" >&2
kill "$harness_pid" "$rrserver_pid" 2>/dev/null
wait 2>/dev/null
exit "$code"
