"""切诊 Sub-Agent：线上无法真实切脉，降级使用自测心率/可穿戴 PPG 数据。

PPG 硬件（或模拟信号）解析结果以高置信度进入证据池；自测心率作为低置信度兜底。
"""
from __future__ import annotations

from ..models.schemas import Evidence
from ..protocol.base import AgentRequest, AgentResponse, Capability, SubAgent
from ..protocol.llm import get_provider, parse_json
from ..protocol.registry import register
from ..skills.toolcall import run_tool_loop
from app.agents.prompts import system_prompt


def _ppg_evidences(ppg: dict, rnd: int) -> list[Evidence]:
    """把 PPG 解析结果转为切诊证据（高置信度）。"""
    out = []
    rate = ppg.get("rate_bpm", 0)
    if rate:
        rate_val = "脉数" if rate >= 90 else ("脉迟" if rate <= 60 else "脉平")
        out.append(Evidence(key="pulse.rate", value=f"{rate:.0f}次/分·{rate_val}",
                            source="切", confidence=0.85, round=rnd))
    for key, field_, conf in (
        ("pulse.position", "depth", 0.8),
        ("pulse.force", "force", 0.8),
        ("pulse.shape", "shape", 0.75),
        ("pulse.rhythm", "rhythm", 0.8),
    ):
        v = ppg.get(field_)
        if v:
            out.append(Evidence(key=key, value=str(v), source="切",
                                confidence=conf, round=rnd))
    return out


@register
class PalpationRuleAgent(SubAgent):
    capability = Capability.PALPATION
    impl_name = "rule"
    description = "PPG/自测心率 -> 脉象证据（PPG 高置信度，自测心率低置信度）"

    async def handle(self, req: AgentRequest) -> AgentResponse:
        # PPG 解析结果优先（硬件/模拟），高置信度
        ppg = req.payload.get("ppg")
        if ppg:
            evs = _ppg_evidences(ppg, req.round)
            qual = ppg.get("signal_quality", 0)
            notes = ppg.get("notes") or "PPG 脉象解析"
            notes = f"{notes}（信号质量 {qual:.0%}）"
            return AgentResponse(capability=self.capability, evidences=evs, notes=notes)

        # 兜底：自测心率
        sr = req.payload.get("self_report") or {}
        hr = sr.get("heart_rate")
        if not hr:
            return AgentResponse(capability=self.capability, status="skip",
                                 notes="未提供心率/脉象，切诊跳过（可接 PPG 手环采样补充）")
        try:
            hr = float(hr)
        except (TypeError, ValueError):
            return AgentResponse(capability=self.capability, status="skip")
        if hr >= 90:
            value = "脉数"
        elif hr <= 55:
            value = "脉迟"
        else:
            value = "脉率正常"
        ev = Evidence(key="pulse.rate", value=value, source="切",
                      confidence=0.5, round=req.round)
        return AgentResponse(capability=self.capability, evidences=[ev],
                             notes=f"自测心率 {hr:.0f} 次/分 → {value}（仅供参考）")


@register
class PalpationLLMAgent(SubAgent):
    capability = Capability.PALPATION
    impl_name = "llm"
    description = "LLM 语义化切诊自述（PPG/自测心率规则作为兜底）"

    async def handle(self, req: AgentRequest) -> AgentResponse:
        text = req.payload.get("text", "")
        if not text:
            return await PalpationRuleAgent().handle(req)
        user = f"请从以下自述中抽取切诊证据，仅输出 JSON：\n{text}"
        try:
            raw, usage = await run_tool_loop(
                get_provider(),
                [
                    {"role": "system", "content": system_prompt(Capability.PALPATION)},
                    {"role": "user", "content": user},
                ],
                req.model, Capability.PALPATION.value,
            )
            data = parse_json(raw)
        except Exception:
            return await PalpationRuleAgent().handle(req)
        evs: list[Evidence] = []
        for e in data.get("evidences", []) or []:
            cat = e.get("category")
            val = e.get("value")
            if cat and val:
                evs.append(Evidence(key=cat, value=val, source="切",
                                    confidence=float(e.get("confidence", 0.5)),
                                    round=req.round))
        if not evs:
            return await PalpationRuleAgent().handle(req)
        return AgentResponse(capability=self.capability, evidences=evs,
                             notes=data.get("notes", ""), meta={"tokens": usage})
