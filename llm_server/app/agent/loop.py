"""Agent 循环：带工具调用的多步推理（ReAct 风格）。

流程：
  1. 把 messages + 工具声明发给 LM Studio；
  2. 若模型返回 tool_calls，则逐一执行工具（注册表/MCP），
     把结果以 tool 消息回填，进入下一轮；
  3. 若模型返回纯文本（无工具调用），或达到最大轮数，则结束。

返回最终文本 + 每轮轨迹（trace）与 token 用量（usage），
便于上层聚合与可观测性。
"""
from __future__ import annotations

import json
import logging
import uuid
from dataclasses import dataclass, field
from typing import Any

from ..config import Settings
from ..provider import LMStudioClient
from ..tools.registry import ToolRegistry

logger = logging.getLogger("llm_server.agent")


@dataclass
class AgentResult:
    content: str = ""
    rounds: int = 0
    usage: dict[str, int] = field(default_factory=dict)
    trace: list[dict[str, Any]] = field(default_factory=list)


def _merge_usage(total: dict[str, int], usage: dict[str, Any]) -> None:
    for key in ("prompt_tokens", "completion_tokens", "total_tokens"):
        val = usage.get(key)
        if isinstance(val, (int, float)):
            total[key] = total.get(key, 0) + int(val)


async def _llm_once(provider: LMStudioClient, model: str, messages: list[dict],
                    tools: list[dict], cfg: Settings,
                    temperature: float = 0.4) -> tuple[str, list[dict], dict[str, Any]]:
    """调用一次 LM Studio，返回 (content, tool_calls, usage)。"""
    body: dict[str, Any] = {"model": model, "messages": messages}
    if tools:
        body["tools"] = tools
        body["tool_choice"] = "auto"
    if temperature is not None:
        body["temperature"] = temperature

    payload = await provider.chat_completions(body)
    usage = payload.get("usage") or {}
    msg = (payload.get("choices") or [{}])[0].get("message") or {}
    content = msg.get("content") or ""
    tool_calls = msg.get("tool_calls") or []
    return content, tool_calls, usage


async def run_agent_loop(
    provider: LMStudioClient,
    registry: ToolRegistry,
    cfg: Settings,
    messages: list[dict],
    model: str | None = None,
    tools: list[dict] | None = None,
    max_rounds: int | None = None,
    temperature: float = 0.4,
) -> AgentResult:
    """执行 agent 循环。

    - ``tools``：可选的额外 OpenAI 工具声明；执行时优先在 registry 中找同名
      handler，找不到则向模型返回「工具不可用」错误（模型会停止调用或换工具）。
    - 最终的工具声明 = registry 全部工具 ∪ 请求方声明的 tools。
    """
    model = model or cfg.default_model
    rounds = max_rounds or cfg.agent_max_rounds
    max_out = cfg.agent_max_tool_output_chars

    work_messages: list[dict] = [dict(m) for m in messages]
    result = AgentResult()
    tool_schemas: dict[str, dict] = {}
    for t in registry.schemas():
        tool_schemas[t["function"]["name"]] = t
    for t in tools or []:
        fn = t.get("function", t)   # 兼容 {type,function} 与裸声明
        name = fn.get("name")
        if name:
            tool_schemas.setdefault(name, {"type": "function", "function": fn})

    combined_tools = list(tool_schemas.values())

    for rnd in range(1, rounds + 1):
        try:
            content, tool_calls, usage = await _llm_once(
                provider, model, work_messages, combined_tools, cfg, temperature)
        except Exception as e:  # noqa: BLE001
            logger.exception("agent 第 %d 轮调用失败", rnd)
            result.content = f"（agent 调用模型失败: {e}）"
            result.rounds = rnd
            return result

        _merge_usage(result.usage, usage)

        if not tool_calls:
            result.content = content
            result.rounds = rnd
            return result

        # 执行工具调用
        assistant_msg: dict[str, Any] = {"role": "assistant", "content": content or ""}
        assistant_msg["tool_calls"] = tool_calls
        work_messages.append(assistant_msg)

        for tc in tool_calls:
            fn = tc.get("function") or {}
            name = fn.get("name", "")
            raw_args = fn.get("arguments") or "{}"
            try:
                arguments = json.loads(raw_args) if isinstance(raw_args, str) else raw_args
            except json.JSONDecodeError:
                arguments = {}
            tool_output = await registry.call(name, arguments)
            tool_output = tool_output[:max_out] if tool_output else "（空结果）"
            work_messages.append({
                "role": "tool",
                "tool_call_id": tc.get("id") or f"call_{uuid.uuid4().hex[:8]}",
                "content": tool_output,
            })
            result.trace.append({
                "round": rnd,
                "tool": name,
                "arguments": arguments,
                "output": tool_output[:500],
            })
            logger.info("agent round=%d tool=%s args=%s", rnd, name, arguments)

    # 达到最大轮数仍未收尾
    result.rounds = rounds
    result.content = "（已达到最大工具调用轮数，请基于已有信息作答）"
    return result
