"""工具注册表：统一管理「可供 agent 调用的工具」。

工具来源：
  1. 内置工具（app/tools/builtin.py）；
  2. 外部 MCP Server 拉取的工具（app/mcp/manager.py）。

每个工具 = JSON Schema 声明（暴露给模型）+ 可执行 handler。
handler 接受 kwargs（由模型按 schema 生成），返回 str 结果。
"""
from __future__ import annotations

import asyncio
import inspect
import logging
from dataclasses import dataclass
from typing import Any, Awaitable, Callable

logger = logging.getLogger("llm_server.tools")

Handler = Callable[..., str | Awaitable[str]]


@dataclass
class Tool:
    name: str
    description: str
    parameters: dict[str, Any]      # JSON Schema（parameters 部分）
    handler: Handler
    source: str = "builtin"         # builtin / mcp:<client>

    def schema(self) -> dict[str, Any]:
        """转成 OpenAI function calling 声明。"""
        return {
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            },
        }

    async def run(self, **kwargs: Any) -> str:
        if inspect.iscoroutinefunction(self.handler):
            return await self.handler(**kwargs)
        result = self.handler(**kwargs)
        if inspect.isawaitable(result):
            return await result
        return str(result)


class ToolRegistry:
    def __init__(self) -> None:
        self._tools: dict[str, Tool] = {}

    def register(self, tool: Tool) -> None:
        if tool.name in self._tools:
            logger.warning("覆盖已有工具: %s", tool.name)
        self._tools[tool.name] = tool

    def unregister(self, name: str) -> None:
        self._tools.pop(name, None)

    def get(self, name: str) -> Tool | None:
        return self._tools.get(name)

    def list(self) -> list[Tool]:
        return list(self._tools.values())

    def schemas(self) -> list[dict[str, Any]]:
        return [t.schema() for t in self._tools.values()]

    async def call(self, name: str, arguments: dict[str, Any]) -> str:
        """执行工具；未注册或执行失败时返回错误信息（模型可见并会自行修正）。"""
        tool = self._tools.get(name)
        if tool is None:
            return f"ERROR: 工具「{name}」未注册/不可用。请勿再调用该工具。"
        try:
            return await asyncio.wait_for(tool.run(**arguments), timeout=30)
        except asyncio.TimeoutError:
            return f"ERROR: 工具「{name}」执行超时（>30s）。"
        except Exception as e:  # noqa: BLE001
            logger.exception("工具执行失败: %s", name)
            return f"ERROR: 工具「{name}」执行失败: {e}"
