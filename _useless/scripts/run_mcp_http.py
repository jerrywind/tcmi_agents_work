"""以独立进程启动 TCM 中医问诊 MCP Server（Streamable HTTP）。

说明：主后端服务已在 `/mcp` 挂载了同一套 MCP Server（见 `app/main.py`），
本脚本用于**不想启动整个后端**、或希望把 MCP 能力**独立部署/扩缩容**的场景，
典型用途是配合远程 Sub-Agent（`routing.yaml` 中 `impl: mcp`）：
把望诊等重负载能力单独部署在带 GPU 的机器上。

用法::

    python scripts/run_mcp_http.py                    # 默认 0.0.0.0:8001，端点 /mcp
    PORT=9000 python scripts/run_mcp_http.py          # 自定义端口
    MCP_ONLY_AGENT_TOOLS=1 python scripts/run_mcp_http.py   # 只暴露 Agent 级工具

客户端连接 ``http://<host>:<port>/mcp``（Streamable HTTP）。
另可用 stdio 传输：``cd backend && python -m app.mcp.server``。
"""
from __future__ import annotations

import contextlib
import os
import sys
from pathlib import Path

# 让 backend 包可被导入（脚本位于项目根的 scripts/ 目录）
sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "backend"))

import uvicorn
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse
from starlette.routing import Mount, Route

from app.config import settings
from app.mcp.server import build_http_app, list_tools

MCP_PATH = "/mcp"


def make_app() -> Starlette:
    # 只暴露 Agent 级工具：适合作为"远程 Sub-Agent 工作节点"部署
    if os.environ.get("MCP_ONLY_AGENT_TOOLS", "").strip().lower() in ("1", "true", "yes"):
        settings.mcp["server"]["expose_session_tools"] = False

    endpoint = build_http_app()

    async def health(_request: Request) -> JSONResponse:
        return JSONResponse({
            "ok": True,
            "mcp": MCP_PATH,
            "transport": "streamable-http",
            "tools": [t.name for t in list_tools()],
        })

    @contextlib.asynccontextmanager
    async def lifespan(_app: Starlette):
        async with endpoint.run():
            yield

    return Starlette(
        routes=[Route("/healthz", health), Mount(MCP_PATH, endpoint)],
        lifespan=lifespan,
    )


if __name__ == "__main__":
    port = int(os.environ.get("PORT", "8001"))
    uvicorn.run(make_app(), host="0.0.0.0", port=port)
