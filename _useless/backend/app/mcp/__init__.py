"""MCP（Model Context Protocol）集成。

本包让项目同时具备两种角色：

- **MCP Server**（``server.py``）：把中医问诊能力暴露给外部 MCP 客户端，
  分会话级与 Agent 级两层工具粒度（``tools/``）。
- **MCP Client**（``client.py``）：连接外部 MCP Server，把其工具接入
  SKILL 体系；配合 ``remote_agent.py`` 还可把某个 capability 整体路由到远程。

设计说明见 ``docs/mcp.md``。
"""
from .client import MCPConnectionError, MCPToolHub, tool_hub

__all__ = ["MCPToolHub", "MCPConnectionError", "tool_hub"]
