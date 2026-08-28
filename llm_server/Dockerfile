# llm_server v2 —— LM Studio 网关 + Agent 中间层（纯 Python，无需模型权重）
#
# 模型推理全部由宿主机 LM Studio 提供；本镜像仅承载
# prompt 优化 / tool calling / MCP / agent 逻辑。
#
# 注意：容器内访问宿主机 LM Studio 需使用 host.docker.internal
# （compose 已通过 extra_hosts 注入），或在运行时用 -e LMSTUDIO_BASE_URL 覆盖。
FROM python:3.11-slim

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY app ./app
# rag 为可选独立组件（默认不随主服务启动），保留以便单独运行
COPY rag ./rag

ENV PYTHONUNBUFFERED=1 \
    LLM_HOST=0.0.0.0 \
    LLM_PORT=8000

EXPOSE 8000

# 健康检查：本服务存活 + 上游 LM Studio 连通性
HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
  CMD python -c "import urllib.request,sys; \
  sys.exit(0 if urllib.request.urlopen('http://127.0.0.1:8000/healthz', timeout=3).status==200 else 1)" \
  || exit 1

CMD ["python", "-m", "app.main"]
