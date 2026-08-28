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


if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host=settings.host, port=settings.port, log_level="info")
