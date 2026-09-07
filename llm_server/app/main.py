"""llm_server 应用入口：LM Studio 网关 + Agent 中间层。

本地开发：
    pip install -r requirements.txt
    python -m app.main            # 等价 uvicorn app.main:app

Docker：
    见 Dockerfile / docker-compose.yml（默认经 host.docker.internal 访问宿主机 LM Studio）。
"""
from __future__ import annotations

import asyncio
import logging
from contextlib import asynccontextmanager

from fastapi import FastAPI

from .config import settings
from .gateway import router
from .runtime import Runtime
from .rag_router import build_rag_router, warmup_rag

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
    # 预热典籍索引放后台：冷读盘可能要十几秒，不能拿来拖慢启动，
    # 但也不能不预热——否则 harness 起来后的第一次探活正好撞上它，被判成不可用。
    asyncio.create_task(warmup_rag())
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

# RAG（中医典籍检索）：此前只作为独立服务存在（`python -m rag serve`），
# 主服务根本没挂载，而 harness 的 rag_endpoint 指向的正是本服务，
# 导致整条 RAG 链路空转。这里挂载为可选子应用：
# 索引未构建或依赖缺失时只降级，不影响网关本身。
#
# 顺序有讲究：gateway 的 `/{path:path}` 兜底路由按注册顺序匹配，RAG 必须在
# 兜底**之前**注册，否则 /rag/* 全被兜底接住返回 404（曾实测静默失效——
# RAG 日志显示已挂载，harness 的 rag 探活却永远不可达）。
# RAG 端点都以 /rag 开头，与 gateway 前缀不冲突，先挂不会抢任何 gateway 路由。
app.include_router(build_rag_router())
app.include_router(router)


if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host=settings.host, port=settings.port, log_level="info")
