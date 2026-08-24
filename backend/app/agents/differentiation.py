"""辨证 Sub-Agent：证据池 -> 候选证候排序 + 置信度 + 证据链。

- rule：基于知识库权重打分（支持证据加分、矛盾证据减分，归一化为置信度）。
- llm：把证据与候选证候交给 LLM 复核排序，失败自动回退规则结果。
"""
from __future__ import annotations

import json

from ..knowledge.syndromes import SYNDROMES
from ..models.schemas import Evidence, Hypothesis
from ..protocol.base import AgentRequest, AgentResponse, Capability, SubAgent
from ..protocol.llm import get_provider, parse_json
from ..protocol.registry import register
from ..skills.toolcall import run_tool_loop
from app.agents.prompts import system_prompt


def score_syndromes(evidences: list[Evidence]) -> list[Hypothesis]:
    ev_map: dict[str, Evidence] = {}
    for ev in evidences:            # 同 key 保留最新一条
        ev_map[ev.key] = ev
    result: list[Hypothesis] = []
    for name, features in SYNDROMES.items():
        total = sum(max(vals.values()) for vals in features.values())
        support, contra = 0.0, 0.0
        supporting, contradicting = [], []
        for key, vals in features.items():
            ev = ev_map.get(key)
            if ev is None:
                continue
            if ev.value in vals:
                support += vals[ev.value] * ev.confidence
                supporting.append(f"{ev.value}（{ev.source}诊）")
            else:
                w = max(vals.values())
                if w >= 1.0:        # 强特征不匹配 → 矛盾证据
                    contra += 0.5 * w * ev.confidence
                    contradicting.append(f"{ev.key}={ev.value} 与本证不符")
        conf = max(0.0, support - contra) / total if total else 0.0
        result.append(Hypothesis(name=name, confidence=round(min(conf, 0.99), 3),
                                 supporting=supporting, contradicting=contradicting))
    result.sort(key=lambda h: h.confidence, reverse=True)
    return result


@register
class DifferentiationRuleAgent(SubAgent):
    capability = Capability.DIFFERENTIATION
    impl_name = "rule"
    description = "知识库加权评分辨证"

    async def handle(self, req: AgentRequest) -> AgentResponse:
        return AgentResponse(capability=self.capability,
                             hypotheses=score_syndromes(req.evidences))


@register
class DifferentiationLLMAgent(SubAgent):
    capability = Capability.DIFFERENTIATION
    impl_name = "llm"
    description = "LLM 辨证复核（规则打分作为先验，失败回退规则）"

    async def handle(self, req: AgentRequest) -> AgentResponse:
        rule_hyps = score_syndromes(req.evidences)
        ev_lines = [f"- {e.key}={e.value}（{e.source}诊, 置信{e.confidence}）"
                    for e in req.evidences]
        prior = [{"name": h.name, "confidence": h.confidence} for h in rule_hyps[:5]]
        prompt = (
            "你是中医辨证助手。根据证据与先验评分，重排候选证候并给出置信度(0~1)。\n"
            f"候选证候（先验）：{json.dumps(prior, ensure_ascii=False)}\n"
            f"证据：\n" + "\n".join(ev_lines) +
            "\n仅输出 JSON：{\"hypotheses\": [{\"name\": str, \"confidence\": float, \"reason\": str}]}，"
            "name 必须来自候选列表。"
            "\n如需校准，可调用 lookup_syndrome_patterns(syndrome) 查询某证候典型表现。"
        )
        raw, usage = await run_tool_loop(
            get_provider(),
            [
                {"role": "system", "content": system_prompt(Capability.DIFFERENTIATION)},
                {"role": "user", "content": prompt},
            ],
            req.model, Capability.DIFFERENTIATION.value)
        data = parse_json(raw)
        items = data.get("hypotheses") or []
        if not items:
            return AgentResponse(capability=self.capability, hypotheses=rule_hyps,
                                 notes="LLM 不可用，使用规则辨证",
                                 meta={"tokens": usage, "degraded": True})
        by_name = {h.name: h for h in rule_hyps}
        merged: list[Hypothesis] = []
        for it in items:
            base = by_name.get(it.get("name", ""))
            if base is None:
                continue
            base.confidence = round(float(it.get("confidence", base.confidence)), 3)
            if it.get("reason"):
                base.supporting = base.supporting + [f"LLM: {it['reason']}"]
            merged.append(base)
        for h in rule_hyps:          # 补齐 LLM 没提到的
            if h not in merged:
                merged.append(h)
        merged.sort(key=lambda h: h.confidence, reverse=True)
        return AgentResponse(capability=self.capability, hypotheses=merged,
                             meta={"tokens": usage})
