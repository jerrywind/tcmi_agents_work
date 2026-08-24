#!/usr/bin/env bash
# llm_server 启动入口：用 llama.cpp 的 llama-server 拉起模型。
# - 文本模型（如 qwen3.6-9B）：可直接纯文本启动；
#   若提供 mmproj 投影文件，则额外获得图文理解（可选）。
# - 视觉模型（如 Qwen3-VL）：原生多模态，无需 mmproj 即可理解图像。
set -euo pipefail

MODEL_PATH="${MODEL_PATH:-/models/qwen3.6-9B.gguf}"
MMPROJ_PATH="${MMPROJ_PATH:-/models/mmproj.gguf}"
HOST="${LLM_HOST:-0.0.0.0}"
PORT="${LLM_PORT:-8000}"
CTX="${CTX_SIZE:-8192}"
GPU="${GPU_LAYERS:-0}"
KEY="${API_KEY:-sk-noauth}"

if [ ! -f "$MODEL_PATH" ]; then
  echo "[llm_server] ERROR: 模型文件不存在: $MODEL_PATH" >&2
  echo "[llm_server] 请将模型 GGUF 权重挂载到该路径（或设置 MODEL_PATH，如 qwen3.6-9B / qwen3-vl）。" >&2
  exit 1
fi

# 组装启动参数；若 mmproj 存在则启用多模态（图文理解）
ARGS=(--model "$MODEL_PATH" --host "$HOST" --port "$PORT" --ctx-size "$CTX" --n-gpu-layers "$GPU" --api-key "$KEY")

if [ -f "$MMPROJ_PATH" ]; then
  echo "[llm_server] 启用 mmproj 多模态： $MMPROJ_PATH"
  ARGS+=(--mmproj "$MMPROJ_PATH")
else
  echo "[llm_server] 未找到 mmproj（$MMPROJ_PATH）：若模型为 Qwen3-VL 等原生多模态模型则无需 mmproj；"
  echo "[llm_server] 若为纯文本模型且需要图文理解，请挂载对应的 mmproj 文件。" >&2
fi

echo "[llm_server] 启动模型: $MODEL_PATH  (host=$HOST port=$PORT ctx=$CTX gpu_layers=$GPU)"

# 启动 RAG 服务（Python 3）：复用本服务 Embedding 与 Qwen3-VL 端点，
# 提供文本 RAG / 图像 RAG / 图文对应 RAG。监听 8080（RAG_PORT）。
RAG_PORT="${RAG_PORT:-8080}"
echo "[llm_server] 启动 RAG 服务 (port=$RAG_PORT) ..."
cd /app && python3 -m rag serve > /var/log/rag.log 2>&1 &
RAG_PID=$!

# 前台启动模型服务；退出时一并结束 RAG 服务
cleanup() {
  echo "[llm_server] 停止 RAG 服务 (pid=$RAG_PID) ..."
  kill "$RAG_PID" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

exec llama-server "${ARGS[@]}"
