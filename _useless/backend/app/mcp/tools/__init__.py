"""MCP 工具集合：按粒度分为两层。

- ``session``：会话级工具（create_consultation / start / answer ...），
  面向"帮我完整跑一次问诊"的对话式客户端，带会话状态。
- ``agents`` ：Agent 级工具（agent_inspection / agent_differentiation ...），
  面向"只借用某项中医原子能力"的调用方，无状态、输入即完整上下文。
"""
from . import agents, session

__all__ = ["agents", "session"]
