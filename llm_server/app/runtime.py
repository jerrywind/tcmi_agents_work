"""Runtime：聚合 LM Studio 客户端 + 工具注册表 + MCP 管理器。

生命周期由 FastAPI lifespan 管理（start/stop）。
"""
from __future__ import annotations

import logging

from .config import Settings
from .mcp import MCPManager
from .provider import LMStudioClient
from .tools.builtin import register_builtin_tools
from .tools.registry import ToolRegistry

logger = logging.getLogger("llm_server")


class Runtime:
    def __init__(self, cfg: Settings) -> None:
        self.cfg = cfg
        self.provider = LMStudioClient(cfg)
        self.tools = ToolRegistry()
        self.mcp = MCPManager(cfg)

    async def start(self) -> None:
        register_builtin_tools(self.tools)
        await self.mcp.start()
        await self.mcp.inject_tools(self.tools)
        logger.info(
            "llm_server 启动完成：上游=%s 默认模型=%s 可用工具=%d",
            self.cfg.lmstudio_base_url, self.cfg.default_model, len(self.tools.list()),
        )

    async def stop(self) -> None:
        await self.mcp.close()
