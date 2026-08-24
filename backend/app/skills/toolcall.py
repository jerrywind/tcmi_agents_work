"""LLM 工具调用循环（function calling）。

当某 capability 下注册了可用工具时，agent 通过本循环让 LLM 多轮调用工具，
最后再以 json_mode 产出结构化结果。无工具时退化为单次 json_mode 调用，
保证与既有行为完全兼容。
"""
from __future__ import annotations

import json
from typing import Any

from .registry import skill_registry

# OpenAI 风格的 tool_call 结构：{"id","type":"function","function":{"name","arguments"}}
DEFAULT_MAX_TOOL_ROUNDS = 3


def _add_usage(total: dict, part: dict | None) -> dict:
    if not part:
        return total
    for k in ("prompt_tokens", "completion_tokens", "total_tokens"):
        total[k] = total.get(k, 0) + int(part.get(k, 0) or 0)
    return total


async def run_tool_loop(provider: Any, messages: list[dict], model: str | None, capability: str,
                        *, max_tool_rounds: int = DEFAULT_MAX_TOOL_ROUNDS) -> tuple[str, dict]:
    """执行带工具调用的对话循环，返回 (最终文本, 累计 token usage)。"""
    model = model or ""
    tools = skill_registry.tools_for(capability)
    usage: dict = {}
    if not tools:
        res, u = await provider.chat(messages, model, json_mode=True)
        _add_usage(usage, u)
        return res, usage

    msgs: list[dict] = [dict(m) for m in messages]
    for _ in range(max_tool_rounds):
        res, u = await provider.chat(msgs, model, json_mode=False, tools=tools)
        _add_usage(usage, u)
        # 部分 provider 在无工具调用时直接返回文本 —— 视作最终结果
        if isinstance(res, str):
            return res, usage
        content = res.get("content") or ""
        calls = res.get("tool_calls") or []
        if not calls:
            return content or "{}", usage
        msgs.append({
            "role": "assistant",
            "content": content,
            "tool_calls": calls,
        })
        for call in calls:
            fn = call.get("function", {})
            name = fn.get("name", "")
            try:
                args = json.loads(fn.get("arguments", "") or "{}")
            except Exception:  # noqa: BLE001
                args = {}
            try:
                result = await skill_registry.run_tool(name, args)
            except Exception as e:  # noqa: BLE001 工具异常不应击穿整个流程
                result = {"error": f"{type(e).__name__}: {e}"}
            msgs.append({
                "role": "tool",
                "tool_call_id": call.get("id"),
                "name": name,
                "content": json.dumps(result, ensure_ascii=False),
            })

    # 工具轮次用尽，做最后一次 json_mode 综合
    res, u = await provider.chat(msgs, model, json_mode=True)
    _add_usage(usage, u)
    return res, usage
