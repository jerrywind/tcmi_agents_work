"""MCP Server：把中医智能问诊能力暴露为标准 MCP 工具。

支持两种传输：
- **stdio**          ：`python -m app.mcp.server`，供 Claude Desktop / Cursor 本地接入
- **Streamable HTTP**：在 FastAPI 的 `/mcp` 路径挂载（见 `app/main.py`），供远端客户端访问

工具分两层粒度（见 `app/mcp/tools/`）：
- 会话级 `tools/session.py`：完整问诊流程，带 cid 状态
- Agent 级 `tools/agents.py`：望/闻/问/切/辨证/治法/安全 单项能力，无状态

两层是否暴露由 `routing.yaml` 的 `mcp.server.expose_*` 控制。
"""
from __future__ import annotations

import json
from contextlib import asynccontextmanager
from typing import Any

from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp.types import TextContent, Tool

from ..config import settings
from .tools import agents as agent_tools
from .tools import session as session_tools

SERVER_NAME = "tcm-consult"
SERVER_INSTRUCTIONS = (
    "中医智能问诊 MCP Server。提供两类工具：\n"
    "1) 会话级：create_consultation -> upload_image/upload_ppg -> start_consultation "
    "-> (get_state / answer_question 循环) -> get_report，完成一次完整问诊；\n"
    "2) Agent 级：agent_inspection/agent_listening/agent_inquiry/agent_palpation/"
    "agent_differentiation/agent_treatment/agent_safety，可单独调用某项中医能力（无状态）。\n"
    "先调用 list_agent_capabilities 可查看全部能力与当前实现。"
)

# 向后兼容：旧代码从本模块导入过这些名字
_VALID_IMG = session_tools.VALID_IMG


def _expose_session() -> bool:
    return bool(settings.mcp["server"].get("expose_session_tools", True))


def _expose_agents() -> bool:
    return bool(settings.mcp["server"].get("expose_agent_tools", True))


def list_tools() -> list[Tool]:
    """按配置组合两层工具清单。"""
    tools: list[Tool] = []
    if _expose_session():
        tools.extend(session_tools.list_tools())
    if _expose_agents():
        tools.extend(agent_tools.list_tools())
    return tools


def _txt(obj: Any) -> list[TextContent]:
    text = json.dumps(obj, ensure_ascii=False, default=str)
    return [TextContent(type="text", text=text)]


async def _dispatch(name: str, args: dict) -> Any:
    """依次交给会话级、Agent 级处理器；返回 ``None`` 表示"不是我的工具"。"""
    if _expose_session():
        result = await session_tools.handle_call(name, args)
        if result is not None:
            return result
    if _expose_agents():
        result = await agent_tools.handle_call(name, args)
        if result is not None:
            return result
    raise ValueError(f"unknown tool: {name}")


async def handle_call(name: str, args: dict) -> list[TextContent]:
    """集中处理工具调用，返回 MCP content 列表。

    统一在此做异常兜底：把错误序列化为 ``{"error": ...}`` 文本返回，
    避免 MCP 协议层因业务异常中断，且保证 stdio 与 HTTP 两种传输行为一致。
    """
    try:
        return _txt(await _dispatch(name, args or {}))
    except Exception as e:  # noqa: BLE001
        return _txt({"error": f"{type(e).__name__}: {e}"})


def build_server() -> Server:
    server = Server(SERVER_NAME, instructions=SERVER_INSTRUCTIONS)

    @server.list_tools()
    async def _list_tools() -> list[Tool]:
        return list_tools()

    @server.call_tool()
    async def _call_tool(name: str, arguments: dict) -> list[TextContent]:
        # handle_call 内部已做异常兜底，始终返回合法 content
        return await handle_call(name, arguments or {})

    return server


# ---------------------------------------------------------------------------
# Streamable HTTP：挂载到 FastAPI
# ---------------------------------------------------------------------------
class StreamableHttpEndpoint:
    """可挂载到 FastAPI 的 MCP Streamable HTTP 端点（可重入）。

    ``StreamableHTTPSessionManager.run()`` 每个实例只能调用一次，因此这里把
    "挂载对象"与"会话管理器"解耦：挂载对象在应用整个生命周期内保持稳定，
    而管理器在每次进入 ``lifespan`` 时新建。这样 uvicorn ``--reload``、
    测试中反复创建 TestClient 等场景都能正常工作。

    与 stdio 复用同一套 ``build_server()`` 工具实现，保证两种传输行为一致。
    """

    def __init__(self) -> None:
        self._manager = None

    @asynccontextmanager
    async def run(self):
        """在 FastAPI lifespan 中调用，启动/停止底层会话管理器。"""
        from mcp.server.streamable_http_manager import StreamableHTTPSessionManager

        manager = StreamableHTTPSessionManager(
            app=build_server(), json_response=False, stateless=True
        )
        self._manager = manager
        try:
            async with manager.run():
                yield self
        finally:
            self._manager = None

    async def __call__(self, scope, receive, send) -> None:
        manager = self._manager
        if manager is None:
            await _send_plain(
                send, 503,
                "MCP session manager 未启动：请确保应用以 lifespan 方式运行".encode(),
            )
            return
        await manager.handle_request(scope, receive, send)


def build_http_app() -> StreamableHttpEndpoint:
    """构建可挂载到 FastAPI 的 MCP Streamable HTTP 端点。"""
    return StreamableHttpEndpoint()


async def _send_plain(send, status: int, body: bytes) -> None:
    await send({"type": "http.response.start", "status": status,
                "headers": [(b"content-type", b"text/plain; charset=utf-8")]})
    await send({"type": "http.response.body", "body": body})


async def run_stdio() -> None:
    server = build_server()
    async with stdio_server() as (read, write):
        await server.run(read, write, server.create_initialization_options())


if __name__ == "__main__":
    import asyncio

    asyncio.run(run_stdio())
