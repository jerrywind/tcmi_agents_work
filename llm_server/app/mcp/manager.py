"""MCP Manager：连接外部 MCP Server，将其工具注入 ToolRegistry。

工具命名规则：`{client.name}_{tool.name}`（OpenAI 工具名只允许字母数字下划线），
避免多 server 同名工具冲突。调用时 manager 依据前缀路由回对应 client。
"""
from __future__ import annotations

import logging
from typing import Any

from ..config import Settings
from ..tools.registry import Tool, ToolRegistry
from .client import MCPClient

logger = logging.getLogger("llm_server.mcp")


class MCPManager:
    def __init__(self, cfg: Settings) -> None:
        self.cfg = cfg
        self._clients: dict[str, MCPClient] = {}

    async def start(self) -> None:
        """启动时连接所有配置的 MCP Server 并注册其工具。

        单个 server 连接失败不阻断启动（降级为可用子集），仅记日志。
        """
        if not self.cfg.enable_mcp or not self.cfg.mcp_clients:
            logger.info("MCP 未启用或无客户端配置（MCP_CLIENTS 为空）")
            return
        for item in self.cfg.mcp_clients:
            name = str(item.get("name", "")).strip()
            url = str(item.get("url", "")).strip()
            if not name or not url:
                logger.warning("MCP 配置缺少 name/url: %s", item)
                continue
            client = MCPClient(name, url, item.get("headers"))
            try:
                await client.initialize()
                tools = await client.list_tools()
                self._clients[name] = client
                logger.info("MCP[%s] 拉取到 %d 个工具", name, len(tools))
            except Exception as e:  # noqa: BLE001
                logger.warning("MCP[%s] 连接失败，跳过: %s", name, e)
                continue

    async def inject_tools(self, registry: ToolRegistry) -> int:
        """把已连接 MCP server 的工具注入 registry，返回注入数量。"""
        count = 0
        for cname, client in self._clients.items():
            try:
                tools = await client.list_tools()
            except Exception as e:  # noqa: BLE001
                logger.warning("MCP[%s] tools/list 失败: %s", cname, e)
                continue
            for t in tools:
                tname = str(t.get("name", "")).strip()
                if not tname:
                    continue
                input_schema = t.get("inputSchema") or {"type": "object", "properties": {}}
                registry.register(Tool(
                    name=f"{cname}_{tname}",
                    description=str(t.get("description", "") or ""),
                    parameters=input_schema,
                    handler=self._make_handler(cname, tname),
                    source=f"mcp:{cname}",
                ))
                count += 1
        logger.info("MCP 工具注入完成，共 %d 个", count)
        return count

    def _make_handler(self, client_name: str, tool_name: str):
        async def _call(**kwargs: Any) -> str:
            client = self._clients.get(client_name)
            if client is None:
                return f"ERROR: MCP 客户端「{client_name}」不可用"
            return await client.call_tool(tool_name, kwargs)
        return _call

    async def close(self) -> None:
        for client in self._clients.values():
            try:
                await client.close()
            except Exception:  # noqa: BLE001
                pass
        self._clients.clear()
