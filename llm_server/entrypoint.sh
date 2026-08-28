#!/usr/bin/env bash
# llm_server v2 启动入口：打印关键配置后启动 uvicorn。
# 模型由宿主机 LM Studio 提供，本服务不做任何模型托管。
set -euo pipefail

echo "[llm_server] LM Studio 网关 + Agent 中间层 v2"
echo "[llm_server] 上游 LM Studio : ${LMSTUDIO_BASE_URL:-http://localhost:11223/v1}"
echo "[llm_server] 默认模型        : ${DEFAULT_MODEL:-google/gemma-4-12b-qat}"
echo "[llm_server] 监听            : ${LLM_HOST:-0.0.0.0}:${LLM_PORT:-8000}"
echo "[llm_server] MCP 客户端      : ${MCP_CLIENTS:-[]}"

exec python -m app.main
