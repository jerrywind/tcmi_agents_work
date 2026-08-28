"""各 Sub-Agent 所需的技能（skill）映射。

用于文档化与可观测：在 `docs/sub_agents.md` 中据此说明每个子智能体的职责、
system prompt 与可调用的工具。运行时实际可用工具由 `app/skills/registry.py` 按
capability 提供，本映射与其保持一致。
"""
from __future__ import annotations

from app.protocol.base import Capability

# capability -> 该 agent 在 LLM 模式下可调用（经 run_tool_loop 注入）的技能名列表
AGENT_SKILLS: dict[Capability, list[str]] = {
    # 望诊：Qwen3-VL 图像理解（舌/面/患处/任意部位）+ 多模态 RAG 检索
    Capability.INSPECTION: ["tcm-vision", "tcm-rag"],
    # 闻诊：声/嗅标准术语与病机参照
    Capability.LISTENING: ["tcm-auscultation"],
    # 问诊：辨证追问聚焦 + 多模态 RAG 检索
    Capability.INQUIRY: ["tcm-inquiry", "tcm-rag"],
    # 切诊：脉象/腹诊标准术语与病机参照
    Capability.PALPATION: ["tcm-palpation"],
    # 辨证：证候典型表现检索 + 多模态 RAG 检索
    Capability.DIFFERENTIATION: ["tcm-reference", "tcm-rag"],
    # 安全：红旗信号分诊指引
    Capability.SAFETY: ["tcm-safety"],
    # 施治：证候治法/单味药知识库 + 辨证食疗 + 多模态 RAG 检索
    Capability.TREATMENT: ["tcm-kb", "tcm-diet", "tcm-rag"],
}


def skills_for(capability: Capability) -> list[str]:
    return AGENT_SKILLS.get(capability, [])
