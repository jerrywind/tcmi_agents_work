"""闻诊 Sub-Agent：从自述文本中提取声音/气味/症状线索（线上场景降级为文本分析）。"""
from __future__ import annotations

from ..knowledge.syndromes import KEYWORD_EVIDENCE
from ..models.schemas import Evidence
from ..protocol.base import AgentRequest, AgentResponse, Capability, SubAgent
from ..protocol.llm import get_provider, parse_json
from ..protocol.registry import register
from ..skills.toolcall import run_tool_loop
from app.agents.prompts import system_prompt


def extract_keyword_evidences(text: str, round_: int, source: str = "闻") -> list[Evidence]:
    found: dict[str, Evidence] = {}
    for keywords, key, value in KEYWORD_EVIDENCE:
        if key in found:
            continue
        if any(kw in text for kw in keywords):
            found[key] = Evidence(key=key, value=value, source=source,  # type: ignore[arg-type]
                                  confidence=0.7, round=round_)
    return list(found.values())


@register
class ListeningRuleAgent(SubAgent):
    capability = Capability.LISTENING
    impl_name = "rule"
    description = "关键词规则：自述文本 -> 结构化证据"

    async def handle(self, req: AgentRequest) -> AgentResponse:
        text = req.payload.get("text", "")
        if not text:
            return AgentResponse(capability=self.capability, status="skip")
        evidences = extract_keyword_evidences(text, req.round)
        return AgentResponse(capability=self.capability, evidences=evidences)


@register
class ListeningLLMAgent(SubAgent):
    capability = Capability.LISTENING
    impl_name = "llm"
    description = "LLM 语义抽取闻诊证据（规则关键词作为兜底）"

    async def handle(self, req: AgentRequest) -> AgentResponse:
        text = req.payload.get("text", "")
        if not text:
            return AgentResponse(capability=self.capability, status="skip")
        user = f"请从以下自述中抽取闻诊证据，仅输出 JSON：\n{text}"
        try:
            raw, usage = await run_tool_loop(
                get_provider(),
                [
                    {"role": "system", "content": system_prompt(Capability.LISTENING)},
                    {"role": "user", "content": user},
                ],
                req.model, Capability.LISTENING.value,
            )
            data = parse_json(raw)
        except Exception:
            return await ListeningRuleAgent().handle(req)
        evs: list[Evidence] = []
        for e in data.get("evidences", []) or []:
            cat = e.get("category")
            val = e.get("value")
            if cat and val:
                evs.append(Evidence(key=cat, value=val, source="闻",
                                    confidence=float(e.get("confidence", 0.6)),
                                    round=req.round))
        if not evs:
            return await ListeningRuleAgent().handle(req)
        return AgentResponse(capability=self.capability, evidences=evs,
                             notes=data.get("notes", ""), meta={"tokens": usage})
