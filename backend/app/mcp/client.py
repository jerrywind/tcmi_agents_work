"""MCP Client：连接外部 MCP Server，把其工具接入本系统的调用体系。

两条接入路径：
1. **SKILL 路径**：外部工具注册为 ``mcp__<server>__<tool>``，本系统的 LLM Agent
   在推理时可直接 function-calling 调用（见 ``skills/registry.py``）。
2. **Sub-Agent 路径**：配合 ``mcp/remote_agent.py``，把某个 capability 整体
   路由到远程 MCP Server（`routing.yaml` 中 ``impl: mcp``）。

用法：
    hub = MCPToolHub()
    await hub.connect(name="weather", transport="stdio", command="python", args=["w.py"])
    await hub.connect(name="calendar", transport="http", url="http://localhost:9000/mcp")
    result = await hub.call("weather", "forecast", {"city": "北京"})
    await hub.close()
"""
from __future__ import annotations

import asyncio
import json
import logging
import os
from contextlib import AsyncExitStack
from typing import Any

from mcp import ClientSession
from mcp.client.sse import sse_client
from mcp.client.stdio import StdioServerParameters, stdio_client

from ..config import settings
from ..skills.registry import skill_registry
from ..skills.types import SkillManifest, ToolSpec

logger = logging.getLogger(__name__)


class MCPConnectionError(RuntimeError):
    """连接外部 MCP Server 失败。"""


class MCPToolHub:
    """管理与多个外部 MCP Server 的连接，并把它们的工具注册进 SKILL 注册表。

    生命周期由 FastAPI lifespan 托管（见 ``app/main.py``）：启动时按
    ``routing.yaml`` 的 ``mcp.clients`` 自动连接，关闭时统一断开。

    每个 Server 使用独立的 ``AsyncExitStack``，因此单个 Server 断开不影响其他连接。
    """

    def __init__(self, call_timeout: float | None = None) -> None:
        self._sessions: dict[str, ClientSession] = {}
        self._tasks: dict[str, asyncio.Task] = {}       # server -> 守护任务
        self._closing: dict[str, asyncio.Event] = {}    # server -> 关闭信号
        self._tools: dict[str, list[str]] = {}          # server -> 原始工具名
        self._meta: dict[str, dict] = {}                # server -> 连接信息
        self._call_timeout = call_timeout
        self._lock = asyncio.Lock()

    @property
    def call_timeout(self) -> float:
        return self._call_timeout if self._call_timeout is not None else settings.mcp_call_timeout

    # ------------------------------------------------------------------
    # 连接
    # ------------------------------------------------------------------
    async def connect(self, name: str, transport: str = "http", **kwargs: Any) -> list[str]:
        """连接一个外部 MCP Server，返回注册进 SKILL 体系的工具名列表。

        :param name: 连接名（唯一），用于工具名前缀 ``mcp__<name>__``
        :param transport: ``http``（Streamable HTTP）| ``sse`` | ``stdio``
        :param kwargs: http/sse 需 ``url``；stdio 需 ``command``，可选 ``args`` / ``env``

        实现说明：MCP 客户端传输基于 anyio task group，其 cancel scope
        必须在**进入它的同一个任务**中退出。因此这里为每个连接启动一个专属
        守护任务：连接的建立、持有与关闭都发生在该任务内，避免
        "Attempted to exit cancel scope in a different task" 错误。
        """
        async with self._lock:
            if name in self._sessions:
                raise MCPConnectionError(f"MCP server '{name}' 已连接")

            ready: asyncio.Future = asyncio.get_running_loop().create_future()
            closing = asyncio.Event()
            task = asyncio.create_task(
                self._maintain(name, transport, kwargs, ready, closing),
                name=f"mcp-client-{name}",
            )
            try:
                tool_names = await ready
            except Exception as exc:
                closing.set()
                await asyncio.gather(task, return_exceptions=True)
                raise MCPConnectionError(f"连接 MCP server '{name}' 失败: {exc}") from exc

            self._tasks[name] = task
            self._closing[name] = closing
            self._meta[name] = {"transport": transport,
                                **{k: v for k, v in kwargs.items() if k != "env"}}
            return tool_names

    async def _maintain(self, name: str, transport: str, kwargs: dict,
                        ready: asyncio.Future, closing: asyncio.Event) -> None:
        """连接守护任务：建立连接 -> 通知就绪 -> 等待关闭信号 -> 就地清理。"""
        try:
            async with AsyncExitStack() as stack:
                session = await self._open_session(stack, transport, **kwargs)
                await asyncio.wait_for(session.initialize(), timeout=self.call_timeout)
                tool_names = await self._register(name, session)
                self._sessions[name] = session
                if not ready.done():
                    ready.set_result(tool_names)
                await closing.wait()
        except Exception as exc:  # noqa: BLE001
            if not ready.done():
                ready.set_exception(exc)
            else:
                logger.warning("MCP server '%s' 连接中断：%s", name, exc)
        finally:
            self._sessions.pop(name, None)

    async def _open_session(self, stack: AsyncExitStack, transport: str, **kwargs: Any) -> ClientSession:
        transport = (transport or "http").lower()
        if transport == "stdio":
            command = kwargs.get("command")
            if not command:
                raise ValueError("stdio 传输需要 command 参数")
            params = StdioServerParameters(
                command=command,
                args=list(kwargs.get("args") or []),
                env={**os.environ, **(kwargs.get("env") or {})},
            )
            read, write = await stack.enter_async_context(stdio_client(params))
        elif transport == "sse":
            url = kwargs.get("url")
            if not url:
                raise ValueError("sse 传输需要 url 参数")
            read, write = await stack.enter_async_context(
                sse_client(url, headers=kwargs.get("headers"))
            )
        elif transport in ("http", "streamable_http", "streamable-http"):
            from mcp.client.streamable_http import streamablehttp_client
            url = kwargs.get("url")
            if not url:
                raise ValueError("http 传输需要 url 参数")
            read, write, _get_session_id = await stack.enter_async_context(
                streamablehttp_client(url, headers=kwargs.get("headers"))
            )
        else:
            raise ValueError(f"不支持的 transport: {transport}（可选 http|sse|stdio）")
        return await stack.enter_async_context(ClientSession(read, write))

    # 兼容旧接口 -------------------------------------------------------
    async def connect_stdio(self, server_name: str, command: str,
                            args: list[str] | None = None,
                            env: dict[str, str] | None = None) -> list[str]:
        return await self.connect(server_name, "stdio", command=command, args=args, env=env)

    async def connect_http(self, server_name: str, url: str) -> list[str]:
        return await self.connect(server_name, "http", url=url)

    async def connect_from_config(self) -> dict[str, Any]:
        """按 ``routing.yaml`` 的 ``mcp.clients`` 批量连接。

        单个连接失败只记录日志并继续，保证应用可正常启动。
        """
        report: dict[str, Any] = {"connected": {}, "failed": {}}
        for cfg in settings.mcp_client_configs():
            name = cfg.pop("name")
            cfg.pop("enabled", None)
            transport = cfg.pop("transport", "http")
            try:
                tools = await self.connect(name, transport, **cfg)
                report["connected"][name] = tools
                logger.info("MCP client '%s' 已连接，接入 %d 个工具", name, len(tools))
            except Exception as exc:  # noqa: BLE001 启动期不因外部依赖失败而中断
                report["failed"][name] = str(exc)
                logger.warning("MCP client '%s' 连接失败：%s", name, exc)
        return report

    # ------------------------------------------------------------------
    # 注册进 SKILL 体系
    # ------------------------------------------------------------------
    async def _register(self, server_name: str, session: ClientSession) -> list[str]:
        resp = await asyncio.wait_for(session.list_tools(), timeout=self.call_timeout)
        specs: list[ToolSpec] = []
        raw_names: list[str] = []
        for t in resp.tools:
            raw_names.append(t.name)
            specs.append(ToolSpec(
                name=f"mcp__{server_name}__{t.name}",
                description=f"[MCP:{server_name}] {t.description or ''}",
                parameters=t.inputSchema or {"type": "object", "properties": {}},
                capability="",  # 对所有 capability 开放
            ))
        handlers = {
            spec.name: self._make_handler(server_name, raw)
            for spec, raw in zip(specs, raw_names)
        }
        skill_registry.register_skill(
            SkillManifest(
                name=f"mcp_{server_name}",
                description=f"外部 MCP Server '{server_name}' 提供的工具",
                tools=specs,
            ),
            handlers,
            source=f"mcp:{server_name}",
        )
        self._tools[server_name] = raw_names
        return [s.name for s in specs]

    def _make_handler(self, server_name: str, tool_name: str):
        async def handler(**kwargs: Any) -> Any:
            return await self.call(server_name, tool_name, kwargs)
        return handler

    # ------------------------------------------------------------------
    # 调用
    # ------------------------------------------------------------------
    async def call(self, server_name: str, tool_name: str, args: dict | None = None) -> Any:
        """调用远端工具并把 MCP content 归一为可 JSON 序列化的结构。"""
        session = self._sessions.get(server_name)
        if session is None:
            raise MCPConnectionError(f"MCP server '{server_name}' 未连接")
        result = await asyncio.wait_for(
            session.call_tool(tool_name, args or {}), timeout=self.call_timeout
        )
        return _normalize_result(result)

    # ------------------------------------------------------------------
    # 查询 / 断开
    # ------------------------------------------------------------------
    @property
    def connected_servers(self) -> list[str]:
        return list(self._tasks.keys())

    def status(self) -> list[dict]:
        return [
            {
                "name": name,
                "transport": self._meta.get(name, {}).get("transport", ""),
                "url": self._meta.get(name, {}).get("url", ""),
                "tools": self._tools.get(name, []),
                "tool_count": len(self._tools.get(name, [])),
                "alive": name in self._sessions,
            }
            for name in self._tasks
        ]

    def is_connected(self, server_name: str) -> bool:
        return server_name in self._sessions

    async def disconnect(self, server_name: str) -> bool:
        """断开单个 Server 并卸载其工具。

        通过关闭信号让守护任务在自己的任务上下文中清理传输资源。
        """
        async with self._lock:
            if server_name not in self._tasks:
                return False
            skill_registry.unload(f"mcp_{server_name}")
            closing = self._closing.pop(server_name, None)
            task = self._tasks.pop(server_name, None)
            self._tools.pop(server_name, None)
            self._meta.pop(server_name, None)
            if closing is not None:
                closing.set()
            if task is not None:
                try:
                    await asyncio.wait_for(asyncio.shield(task), timeout=self.call_timeout)
                except (TimeoutError, asyncio.TimeoutError):
                    task.cancel()
                    await asyncio.gather(task, return_exceptions=True)
                except Exception as exc:  # noqa: BLE001 断开异常不应向上传播
                    logger.warning("关闭 MCP server '%s' 时出错：%s", server_name, exc)
            self._sessions.pop(server_name, None)
            return True

    async def close(self) -> None:
        for name in list(self._tasks.keys()):
            await self.disconnect(name)


def _normalize_result(result: Any) -> Any:
    """把 MCP CallToolResult 的 content 列表归一为 Python 对象。

    优先解析为 JSON（本项目 Server 侧统一以 JSON 文本返回），
    非 JSON 文本则包装为 ``{"result": text}``。
    """
    parts: list[str] = []
    for c in getattr(result, "content", None) or []:
        if getattr(c, "type", None) == "text":
            parts.append(c.text)
        else:
            parts.append(str(getattr(c, "data", c)))
    text = "\n".join(parts)
    if not text:
        return {}
    try:
        return json.loads(text)
    except ValueError:
        return {"result": text}


# 进程内单例：由 FastAPI lifespan 托管
tool_hub = MCPToolHub()
