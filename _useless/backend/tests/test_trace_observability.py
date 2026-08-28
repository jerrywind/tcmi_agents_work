"""T1.4 验证：trace 响应含 per-round token 用量与降级标注。

- rule 模式（默认）下：无 LLM，期望 impl==实际 impl，不标记降级，tokens 为 None。
- llm 路由但无 API Key 时：LLM agent 运行时回退规则兜底（meta.degraded=True），
  trace 应标记 degraded=True 并给出降级原因。

为避免在进程内重新 import app 导致的跨模块类身份不一致，llm 场景通过
monkeypatch `settings.route_of` 对 treatment.plan 返回 llm 配置，复用同一 app 实例。
"""
import os

import pytest

from app.core.orchestrator import orchestrator
from app.models.schemas import Consultation, Hypothesis, Patient, Report
from app.protocol.base import Capability
from app.protocol.llm import MockProvider


def _make_consultation(**kw):
    if "patient" not in kw:
        kw["patient"] = Patient()
    if "report" not in kw:
        kw["report"] = Report()
    return Consultation(**kw)


async def test_trace_rule_mode_no_degradation():
    c = _make_consultation(complaint="x", gender="男")
    c.status = "planning"
    # 给定辨证结论以驱动 treatment.plan 进入 LLM/规则调用路径
    c.hypotheses = [Hypothesis(name="脾胃湿热", confidence=0.7, supporting=[], contradicting=[])]
    c.report = Report(syndromes=list(c.hypotheses))
    await orchestrator._treatment_step(c)
    assert c.trace, "trace 不应为空"
    for entry in c.trace:
        # rule 模式：期望实现与实际一致，不标记降级
        assert entry.get("degraded") in (False, None)
        assert entry.get("degraded_reason") is None
        # rule 无 LLM token 用量
        assert entry.get("tokens") is None
        assert "impl" in entry and "requested_impl" in entry


async def test_trace_llm_routing_marked_degraded_without_impl(monkeypatch):
    from app.config import settings
    original = settings.route_of

    def _patched(cap: str) -> dict:
        if cap == Capability.TREATMENT.value:
            return {"impl": "llm", "model": "text-default",
                    "options": {"max_questions": 2}}
        return original(cap)

    monkeypatch.setattr(settings, "route_of", _patched)
    # 确定性注入：本用例验证「llm 路由但无可用实现时降级」，直接让 treatment agent
    # 使用 MockProvider（不依赖全局 API Key / provider 缓存状态，避免被其他测试污染）。
    import sys
    from app.agents.treatment import TreatmentLLMAgent
    tmod = sys.modules[TreatmentLLMAgent.__module__]
    monkeypatch.setattr(tmod, "get_provider", lambda: MockProvider())

    c = _make_consultation(complaint="x", gender="男")
    c.status = "planning"
    c.hypotheses = [Hypothesis(name="脾胃湿热", confidence=0.7, supporting=[], contradicting=[])]
    c.report = Report(syndromes=list(c.hypotheses))
    await orchestrator._treatment_step(c)
    # 无 API Key 时，LLM agent 运行时回退规则兜底，trace 应标注降级
    degraded = [e for e in c.trace if e.get("degraded")]
    assert degraded, "llm 路由在无 API Key 时应标记 degraded"
    for e in degraded:
        assert e["degraded_reason"]
        assert "LLM" in e["degraded_reason"]
        assert "规则" in e["degraded_reason"]
