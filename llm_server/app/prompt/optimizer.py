"""Prompt 优化：在透传请求给 LM Studio 前做无损/低损压缩。

目标：
1. 合并相邻同角色消息、剔除空消息（减少冗余轮次）；
2. 超长单条内容做「保首保尾」截断；
3. 总预算超限时，优先裁剪最旧的 assistant/中间轮次，保留 system 与最新 user。
4. 未显式提供 system 时注入默认 system 提示（可配置关闭）。

全部操作只基于字符统计（中文约 1 token/字，英文约 4 字符/token），
不引入分词器依赖，保证轻量。
"""
from __future__ import annotations

import logging
import math

from ..config import Settings

logger = logging.getLogger("llm_server.prompt")

MAX_SINGLE_MSG_CHARS = 8000          # 单条消息上限
HEAD_KEEP = 4000                     # 单条超长时保留头部
TAIL_KEEP = 2000                     # 单条超长时保留尾部
SYSTEM_MAX_CHARS = 2000              # system 提示上限


def estimate_tokens(text: str) -> int:
    """粗略 token 估算：中文约 1 字/token，英文约 4 字符/token。"""
    if not text:
        return 0
    cjk = sum(1 for ch in text if "\u4e00" <= ch <= "\u9fff")
    other = max(0, len(text) - cjk)
    return cjk + math.ceil(other / 4)


def _clip(text: str, head: int = HEAD_KEEP, tail: int = TAIL_KEEP) -> str:
    if len(text) <= head + tail + 40:
        return text
    return text[:head] + f"\n……[已截断 {len(text) - head - tail} 字符]……\n" + text[-tail:]


def _default_system(cfg: Settings) -> str:
    return cfg.prompt_system_brief


def optimize_chat_body(body: dict, cfg: Settings) -> tuple[dict, dict]:
    """优化 /v1/chat/completions 的请求体，返回 (新 body, 统计信息)。"""
    stats = {"before_msgs": 0, "after_msgs": 0, "before_chars": 0,
             "after_chars": 0, "clipped": 0, "merged": 0}

    messages: list[dict] = list(body.get("messages") or [])
    if not messages:
        return body, stats

    stats["before_msgs"] = len(messages)
    stats["before_chars"] = sum(len(str(m.get("content") or "")) for m in messages)

    # 1) 注入默认 system（仅当无任何 system 消息）
    if not any(m.get("role") == "system" for m in messages):
        messages.insert(0, {"role": "system", "content": _default_system(cfg)})
        stats["after_msgs"] += 1

    # 2) 合并相邻同角色（跳过 system 与 tool）
    merged: list[dict] = []
    for m in messages:
        role = m.get("role")
        content = m.get("content")
        if content is None or content == "":
            continue
        if (role in ("user", "assistant") and merged and
                merged[-1].get("role") == role and
                merged[-1].get("name") == m.get("name")):
            merged[-1]["content"] = f"{merged[-1]['content']}\n\n{content}"
            stats["merged"] += 1
        else:
            merged.append(dict(m))

    # 3) system 截断 + 非 system 超长保首保尾
    for m in merged:
        if m.get("role") == "system":
            if len(m["content"]) > SYSTEM_MAX_CHARS:
                m["content"] = m["content"][:SYSTEM_MAX_CHARS] + "……[system 已精简]"
                stats["clipped"] += 1
        else:
            content = m["content"]
            if isinstance(content, str) and len(content) > MAX_SINGLE_MSG_CHARS:
                m["content"] = _clip(content)
                stats["clipped"] += 1

    # 4) 总预算超限：从最旧开始丢弃「可丢」消息（保留 system + 最后一条 user）
    total = sum(len(str(m.get("content") or "")) for m in merged)
    if total > cfg.prompt_max_chars:
        budget = cfg.prompt_max_chars
        keep_idx = {i for i, m in enumerate(merged) if m.get("role") == "system"}
        # 最后一条 user 必须保留（这是当前提问）
        for i in range(len(merged) - 1, -1, -1):
            if merged[i].get("role") == "user":
                keep_idx.add(i)
                break
        current = total
        for i in range(len(merged)):
            if i in keep_idx or current <= budget:
                continue
            cost = len(str(merged[i].get("content") or ""))
            # 只丢弃中间轮次；若它是最后一条 user 之前的历史，可以丢
            if merged[i].get("role") in ("user", "assistant", "tool"):
                merged[i]["content"] = "…（历史已省略）…"
                current = max(0, current - cost + len("…（历史已省略）…"))
                stats["clipped"] += 1
            if current <= budget:
                break

    messages = merged
    stats["after_msgs"] = len(messages)
    stats["after_chars"] = sum(len(str(m.get("content") or "")) for m in messages)

    body = dict(body)
    body["messages"] = messages
    return body, stats
