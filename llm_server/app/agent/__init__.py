"""agent 子包：多步工具调用循环（plan → act → observe）。"""
from .loop import AgentResult, run_agent_loop

__all__ = ["AgentResult", "run_agent_loop"]
