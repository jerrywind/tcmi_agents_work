"""Runtime：聚合 LM Studio 客户端 + 工具注册表 + MCP 管理器 + rrserver 注册。

生命周期由 FastAPI lifespan 管理（start/stop）。
"""
from __future__ import annotations

import logging

from .config import Settings
from .mcp import MCPManager
from .provider import LMStudioClient
from .rrclient import Registrar
from .tools.builtin import register_builtin_tools
from .tools.registry import ToolRegistry

logger = logging.getLogger("llm_server")


class Runtime:
    def __init__(self, cfg: Settings) -> None:
        self.cfg = cfg
        self.provider = LMStudioClient(cfg)
        self.tools = ToolRegistry()
        self.mcp = MCPManager(cfg)
        # rrserver 注册与心跳（未配置 RR_SERVER_BASE 时不启用）
        self.registrar = Registrar(cfg)

    async def start(self) -> None:
        register_builtin_tools(self.tools)
        await self.mcp.start()
        await self.mcp.inject_tools(self.tools)
        # 注册失败不阻塞启动：Registrar 会在后台按退避间隔重试
        await self.registrar.start()
        logger.info(
            "llm_server 启动完成：上游=%s 默认模型=%s 可用工具=%d",
            self.cfg.lmstudio_base_url, self.cfg.default_model, len(self.tools.list()),
        )

    async def stop(self) -> None:
        await self.registrar.stop()
        await self.mcp.close()
