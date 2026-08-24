# llm_server：基于 llama.cpp 的可部署模型服务（OpenAI 兼容 API）
#
# 默认模型：qwen3.6-9B（文本）/ Qwen3-VL（视觉，原生多模态，无需 mmproj）。
# 镜像内置 llama.cpp 的 `llama-server`，并通过 entrypoint.sh 读取环境变量启动。
#
# 同一镜像还内置 Python 3 RAG 服务（见 rag/）：复用本服务的 Embedding 与 Qwen3-VL
# 端点，提供文本 RAG / 图像 RAG / 图文对应 RAG。
#
# 变体：
#   - 默认（本 Dockerfile）：CPU 构建，可在任意机器运行（速度受限）。
#   - GPU（CUDA）：将 FROM 改为 ghcr.io/ggml-org/llama.cpp:server-cuda 即可启用 --n-gpu-layers。
FROM ghcr.io/ggml-org/llama.cpp:server

# ---- Python 3 RAG 依赖 ----
RUN apt-get update && apt-get install -y --no-install-recommends python3 python3-pip \
    && rm -rf /var/lib/apt/lists/*
COPY requirements.txt /rag-requirements.txt
RUN python3 -m pip install --no-cache-dir -r /rag-requirements.txt

ENV MODEL_DIR=/models \
    MODEL_PATH=/models/qwen3.6-9B.gguf \
    MMPROJ_PATH=/models/mmproj.gguf \
    LLM_HOST=0.0.0.0 \
    LLM_PORT=8000 \
    CTX_SIZE=8192 \
    GPU_LAYERS=0 \
    API_KEY=sk-noauth \
    RAG_DATA_DIR=/data/rag \
    RAG_PORT=8080

COPY entrypoint.sh /entrypoint.sh
COPY rag /app/rag
RUN chmod +x /entrypoint.sh

EXPOSE 8000 8080

ENTRYPOINT ["/entrypoint.sh"]
