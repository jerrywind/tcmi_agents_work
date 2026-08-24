"""安全 Sub-Agent：每轮检查红旗症状，命中即建议中断问诊线下就医。"""
from __future__ import annotations

from ..knowledge.syndromes import RED_FLAGS
from ..protocol.base import AgentRequest, AgentResponse, Alert, Capability, SubAgent
from ..protocol.llm import get_provider, parse_json
from ..protocol.registry import register
from ..skills.toolcall import run_tool_loop
from app.agents.prompts import system_prompt


@register
class SafetyRuleAgent(SubAgent):
    capability = Capability.SAFETY
    impl_name = "rule"
    description = "红旗症状关键词扫描"

    async def handle(self, req: AgentRequest) -> AgentResponse:
        text = req.payload.get("text", "")
        alerts: list[Alert] = []
        for kw, reason in RED_FLAGS:
            if kw in text:
                alerts.append(Alert(
                    level="danger", reason=f"检测到「{kw}」：{reason}",
                    advice="该症状超出线上问诊范围，请立即前往医院急诊/相应科室就诊。",
                ))
        return AgentResponse(capability=self.capability, alerts=alerts)


@register
class SafetyLLMAgent(SubAgent):
    capability = Capability.SAFETY
    impl_name = "llm"
    description = "LLM 语义识别红旗 + 规则关键词双重兜底（更不易漏报）"

    async def handle(self, req: AgentRequest) -> AgentResponse:
        text = req.payload.get("text", "")
        # 规则关键词扫描作为强制安全网，永远执行
        rule_resp = await SafetyRuleAgent().handle(req)
        alerts: list[Alert] = list(rule_resp.alerts)
        # LLM 语义识别（无模型/异常时静默跳过，不阻塞规则结果）
        usage: dict = {}
        if text:
            try:
                raw, usage = await run_tool_loop(
                    get_provider(),
                    [
                        {"role": "system", "content": system_prompt(Capability.SAFETY)},
                        {"role": "user", "content": text},
                    ],
                    req.model, Capability.SAFETY.value,
                )
                data = parse_json(raw)
                for a in data.get("alerts", []) or []:
                    lvl = a.get("level")
                    sig = a.get("signal")
                    det = a.get("detail", "")
                    if lvl and sig:
                        alerts.append(Alert(
                            level=("danger" if lvl == "urgent" else "warning"),
                            reason=f"语义识别到「{sig}」：{det}",
                            advice="建议尽快线下就诊评估。",
                        ))
            except Exception:
                pass
        # 去重（按 reason）
        seen: set[str] = set()
        uniq: list[Alert] = []
        for al in alerts:
            if al.reason in seen:
                continue
            seen.add(al.reason)
            uniq.append(al)
        return AgentResponse(capability=self.capability, alerts=uniq,
                             meta={"tokens": usage})
