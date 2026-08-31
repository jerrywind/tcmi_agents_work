"""llm_server 应用入口：LM Studio 网关 + Agent 中间层。

本地开发：
    pip install -r requirements.txt
    python -m app.main            # 等价 uvicorn app.main:app

Docker：
    见 Dockerfile / docker-compose.yml（默认经 host.docker.internal 访问宿主机 LM Studio）。
"""
from __future__ import annotations

import logging
from contextlib import asynccontextmanager

from fastapi import FastAPI

from .config import settings
from .gateway import router
from .runtime import Runtime
from .rag_router import build_rag_router

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s [%(name)s] %(message)s",
)
logger = logging.getLogger("llm_server")


@asynccontextmanager
async def lifespan(app: FastAPI):
    runtime = Runtime(settings)
    app.state.runtime = runtime
    await runtime.start()
    yield
    await runtime.stop()


app = FastAPI(
    title="llm_server · LM Studio 网关 + Agent 中间层",
    version="2.0.0",
    description=(
        "模型由 LM Studio 提供（默认 http://localhost:11223/v1）；本服务提供 "
        "prompt 优化 / tool calling / MCP / agent 实现，并对下游保持 OpenAI 兼容。"
    ),
    lifespan=lifespan,
)
app.include_router(router)

# RAG（中医典籍检索）：此前只作为独立服务存在（`python -m rag serve`），
# 主服务根本没挂载，而 harness 的 rag_endpoint 指向的正是本服务，
# 导致整条 RAG 链路空转。这里挂载为可选子应用：
# 索引未构建或依赖缺失时只降级，不影响网关本身。
app.include_router(build_rag_router())


if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host=settings.host, port=settings.port, log_level="info")
