"""极简 MCP Client（Streamable HTTP，2025-03-26 协议）。

不依赖官方 mcp SDK，直接用 JSON-RPC 2.0 over HTTP：
  - initialize / notifications/initialized
  - tools/list
  - tools/call
响应可能为 JSON 或 text/event-stream（SSE），统一解析。

适用对象：支持「Streamable HTTP」的 MCP Server（如官方参考实现、
`uvx mcp-server-*` 等）。SSE 传统传输暂不支持（如需可扩展）。
"""
from __future__ import annotations

import json
import logging
from typing import Any

import httpx

logger = logging.getLogger("llm_server.mcp")

MCP_PROTOCOL_VERSION = "2025-03-26"
_TIMEOUT = 30


def _parse_sse(text: str) -> dict[str, Any] | None:
    """从 SSE 文本中提取最后一条 data 字段并解析 JSON。"""
    data_parts: list[str] = []
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("data:"):
            data_parts.append(line[5:].strip())
        elif line.startswith("data="):
            data_parts.append(line[5:].strip())
    if not data_parts:
        return None
    try:
        return json.loads(data_parts[-1])
    except json.JSONDecodeError:
        # 某些实现以 JSON 串跨多行 data 输出，尝试整体拼接
        try:
            return json.loads("\n".join(data_parts))
        except json.JSONDecodeError:
            return None


class MCPClient:
    def __init__(self, name: str, url: str, headers: dict[str, str] | None = None) -> None:
        self.name = name
        self.url = url.rstrip("/")
        self.headers = {k: str(v) for k, v in (headers or {}).items()}
        self._id = 0
        self._initialized = False
        self._session_id: str | None = None

    def _next_id(self) -> int:
        self._id += 1
        return self._id

    async def _rpc(self, method: str, params: dict[str, Any] | None = None,
                   notify: bool = False) -> dict[str, Any]:
        payload: dict[str, Any] = {"jsonrpc": "2.0"}
        if not notify:
            payload["id"] = self._next_id()
        payload["method"] = method
        if params is not None:
            payload["params"] = params

        headers = {
            **self.headers,
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        }
        if self._session_id:
            headers["Mcp-Session-Id"] = self._session_id

        async with httpx.AsyncClient(timeout=_TIMEOUT) as client:
            r = await client.post(self.url, json=payload, headers=headers)
            r.raise_for_status()
            if sid := r.headers.get("Mcp-Session-Id"):
                self._session_id = sid

        content_type = r.headers.get("content-type", "")
        if "text/event-stream" in content_type:
            parsed = _parse_sse(r.text)
            if parsed is None:
                raise RuntimeError(f"MCP SSE 响应无法解析（{self.name}）: {r.text[:200]}")
            return parsed
        return r.json()

    async def initialize(self) -> None:
        resp = await self._rpc("initialize", {
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "tcm-llm-gateway", "version": "2.0.0"},
        })
        # 会话建立后发送 initialized 通知（无 id）
        await self._rpc("notifications/initialized", notify=True)
        server_info = (resp.get("result") or {}).get("serverInfo", {})
        logger.info("MCP[%s] 已连接: %s", self.name, server_info)
        self._initialized = True

    async def list_tools(self) -> list[dict[str, Any]]:
        resp = await self._rpc("tools/list")
        return (resp.get("result") or {}).get("tools", [])

    async def call_tool(self, name: str, arguments: dict[str, Any]) -> str:
        resp = await self._rpc("tools/call", {"name": name, "arguments": arguments or {}})
        result = resp.get("result") or {}
        if resp.get("error"):
            return f"ERROR: MCP 工具调用失败: {resp['error']}"
        parts: list[str] = []
        for content in result.get("content", []):
            ctype = content.get("type")
            if ctype == "text":
                parts.append(str(content.get("text", "")))
            elif ctype == "image":
                parts.append("[image]")
            elif ctype == "resource":
                parts.append(str(content.get("uri", "[resource]")))
        if parts:
            return "\n".join(parts)
        return json.dumps(result, ensure_ascii=False) if result else "（工具无输出）"

    async def close(self) -> None:
        self._initialized = False
        self._session_id = None
