"""LLM 实现路径测试：用 FakeProvider 注入 JSON，覆盖 rule 之外的 LLM 分支与 provider 工厂。"""
import json

import pytest

from app.models.schemas import Evidence, Report
from app.protocol.base import Capability
from app.protocol.llm import (
    MockProvider, OpenAICompatProvider, get_provider, parse_json,
)
from app.skills.registry import skill_registry
from app.skills.toolcall import run_tool_loop
from app.skills.types import SkillManifest, ToolSpec

from app.agents.differentiation import DifferentiationLLMAgent
from app.agents.inspection import InspectionVisionAgent
from app.agents.treatment import TreatmentLLMAgent


class FakeProvider:
    def __init__(self, text):
        self.text = text

    async def chat(self, messages, model=None, json_mode=False, tools=None):
        # T1.4 起 chat 返回 (content, usage) 二元组
        return self.text, {}

    @property
    def name(self):
        return "FakeProvider"


def ev(key, value, conf=1.0):
    return Evidence(key=key, value=value, source="闻", confidence=conf)


# ---------------- provider 工厂与解析 ----------------
def test_get_provider_default_mock(monkeypatch):
    import app.protocol.llm as llm_mod
    monkeypatch.setattr(llm_mod, "_provider", None)
    monkeypatch.setitem(llm_mod.settings.llm, "base_url", "")
    monkeypatch.setenv("TCM_LLM_API_KEY", "")
    assert isinstance(get_provider(), MockProvider)


def test_get_provider_openai_when_configured(monkeypatch):
    import app.protocol.llm as llm_mod
    monkeypatch.setattr(llm_mod, "_provider", None)
    monkeypatch.setitem(llm_mod.settings.llm, "base_url", "http://x/v1")
    monkeypatch.setenv("TCM_LLM_API_KEY", "sk-test")
    p = get_provider()
    assert isinstance(p, OpenAICompatProvider)
    assert p.name == "OpenAICompatProvider"


def test_parse_json_robust():
    assert parse_json("```json\n{\"a\": 1}\n```") == {"a": 1}
    assert parse_json("前缀 {\"b\": 2} 后缀") == {"b": 2}
    assert parse_json("无 json") == {}


# ---------------- 辨证 LLM 成功路径 ----------------
async def test_differentiation_llm_success(monkeypatch, build_req):
    fp = FakeProvider(json.dumps(
        {"hypotheses": [{"name": "脾胃湿热", "confidence": 0.88, "reason": "湿热内蕴"}]}))
    monkeypatch.setattr("app.agents.differentiation.get_provider", lambda: fp)

    evs = [ev("thirst", "口苦"), ev("smell", "口臭"), ev("stool", "粘滞不爽"),
           ev("head_body", "肢体困重"), ev("appetite", "食欲不振")]
    req = build_req(Capability.DIFFERENTIATION, evidences=evs)
    resp = await DifferentiationLLMAgent().handle(req)

    top = resp.hypotheses[0]
    assert top.name == "脾胃湿热"
    assert top.confidence == 0.88
    assert any("LLM:" in s for s in top.supporting)


# ---------------- 望诊视觉 LLM 成功路径 ----------------
async def test_inspection_vision_success(monkeypatch, build_req):
    fp = FakeProvider(json.dumps(
        {"findings": [{"part": "tongue.body", "value": "红", "confidence": 0.8}],
         "summary": "舌红"}))
    monkeypatch.setattr("app.agents.inspection.get_provider", lambda: fp)

    req = build_req(Capability.INSPECTION,
                    payload={"images": [{"type": "tongue", "path": "/x.jpg"}]})
    resp = await InspectionVisionAgent().handle(req)
    assert resp.status == "ok"
    assert any(e.key == "tongue.body" for e in resp.evidences)


# ---------------- 诊疗方案 LLM 成功路径 ----------------
async def test_treatment_llm_success_plans(monkeypatch, build_req):
    fp = FakeProvider(json.dumps({"plans": [
        {"category": "中药方剂", "title": "X方", "detail": "d",
         "rationale": "r", "note": "", "priority": 1}]}))
    monkeypatch.setattr("app.agents.treatment.get_provider", lambda: fp)

    qa = [{"key": "treat.herb_form", "value": "可煎药"},
          {"key": "treat.external", "value": "接受"}]
    req = build_req(Capability.TREATMENT, payload={"diagnoses": ["脾胃湿热"], "qa": qa})
    resp = await TreatmentLLMAgent().handle(req)
    assert resp.plans
    assert resp.plans[0].category == "中药方剂"


async def test_treatment_llm_success_ask(monkeypatch, build_req):
    fp = FakeProvider(json.dumps(
        {"ask": {"key": "treat.herb_form", "text": "煎药方式？",
                 "options": ["可煎药", "免煎颗粒"]}}))
    monkeypatch.setattr("app.agents.treatment.get_provider", lambda: fp)

    req = build_req(Capability.TREATMENT, payload={"diagnoses": ["脾胃湿热"], "qa": []})
    resp = await TreatmentLLMAgent().handle(req)
    assert resp.question is not None
    assert resp.question.key == "treat.herb_form"


# ---------------- OpenAI 兼容 Provider 真实调用路径 ----------------
async def test_openai_compat_provider_chat(monkeypatch):
    class _Resp:
        def raise_for_status(self) -> None:
            return None

        def json(self) -> dict:
            return {"choices": [{"message": {"content": "hi"}}]}

    class _Client:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *exc):
            return False

        async def post(self, *args, **kwargs):
            # json_mode 时必须带上 response_format
            assert kwargs["json"]["response_format"] == {"type": "json_object"}
            assert kwargs["headers"]["Authorization"] == "Bearer sk"
            return _Resp()

    import httpx

    monkeypatch.setattr(httpx, "AsyncClient", lambda **kw: _Client())
    p = OpenAICompatProvider("http://x/v1/", "sk")
    out, usage = await p.chat([{"role": "user", "content": "hi"}], model="gpt", json_mode=True)
    assert out == "hi"
    assert isinstance(usage, dict)


def test_parse_json_invalid_substring_returns_empty():
    # 包含 {..} 但子串不是合法 JSON -> 进入内层 except 兜底
    assert parse_json("abc {not json} def") == {}


# ---------------- OpenAI 兼容 Provider 工具调用路径 ----------------
async def test_openai_compat_provider_chat_with_tools(monkeypatch):
    class _Resp:
        def raise_for_status(self) -> None:
            return None

        def json(self) -> dict:
            return {"choices": [{"message": {
                "content": "",
                "tool_calls": [{"id": "1", "type": "function",
                                "function": {"name": "x", "arguments": "{}"}}],
            }}]}

    class _Client:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *exc):
            return False

        async def post(self, *args, **kwargs):
            assert "tools" in kwargs["json"]
            assert kwargs["json"].get("tool_choice") == "auto"
            return _Resp()

    import httpx

    monkeypatch.setattr(httpx, "AsyncClient", lambda **kw: _Client())
    p = OpenAICompatProvider("http://x/v1/", "sk")
    out, usage = await p.chat([{"role": "user", "content": "hi"}], model="gpt",
                       tools=[{"type": "function", "function": {"name": "x"}}])
    assert isinstance(out, dict)
    assert out["tool_calls"][0]["function"]["name"] == "x"
    assert isinstance(usage, dict)


# ---------------- 工具调用循环 ----------------
async def test_run_tool_loop_falls_back_without_tools():
    # 该 capability 下无工具 -> 退化为单次 json_mode 调用
    class _P:
        async def chat(self, messages, model=None, json_mode=False, tools=None):
            # T1.4 起 chat 返回 (content, usage)
            return "PLAINS", {}

    out, usage = await run_tool_loop(_P(), [{"role": "user", "content": "go"}],
                                     "m", "diagnosis.safety")
    assert out == "PLAINS"
    assert usage == {}


async def test_run_tool_loop_executes_tools():
    calls: list[int] = []

    async def my_tool(x: int = 0):
        calls.append(x)
        return {"doubled": x * 2}

    manifest = SkillManifest(
        name="test-skill", version="0.0.1",
        tools=[ToolSpec(
            name="my_tool", description="t",
            parameters={"type": "object",
                        "properties": {"x": {"type": "integer"}},
                        "required": ["x"]},
            capability="treatment.plan")],
    )
    skill_registry.register_skill(manifest, {"my_tool": my_tool})
    try:
        class _P:
            def __init__(self) -> None:
                self.n = 0

            async def chat(self, messages, model=None, json_mode=False, tools=None):
                # T1.4 起 chat 返回 (content, usage)
                self.n += 1
                if self.n == 1:
                    return {"content": "", "tool_calls": [{
                        "id": "c1", "type": "function",
                        "function": {"name": "my_tool", "arguments": json.dumps({"x": 3})}}]}, {}
                return "FINAL", {}

        out, usage = await run_tool_loop(_P(), [{"role": "user", "content": "go"}],
                                         "m", "treatment.plan")
        assert out == "FINAL"
        assert calls == [3]
    finally:
        skill_registry.unload("test-skill")


async def test_run_tool_loop_empty_tool_calls():
    # 工具模式下 LLM 直接返回内容（无工具调用）-> 视作最终结果
    async def my_tool():
        return {}

    skill_registry.register_skill(
        SkillManifest(name="rt-empty", version="0.0.1", tools=[ToolSpec(
            name="my_tool", description="t", parameters={}, capability="treatment.plan")]),
        {"my_tool": my_tool},
    )
    try:
        class _P:
            async def chat(self, messages, model=None, json_mode=False, tools=None):
                # T1.4 起 chat 返回 (content, usage)
                return {"content": "FINAL", "tool_calls": []}, {}

        out, usage = await run_tool_loop(_P(), [{"role": "user", "content": "go"}],
                                         "m", "treatment.plan")
        assert out == "FINAL"
    finally:
        skill_registry.unload("rt-empty")


async def test_run_tool_loop_provider_returns_str_in_tool_mode():
    # 工具模式下 provider 直接返回文本（非 dict）-> 立即作为最终结果
    async def my_tool():
        return {}

    skill_registry.register_skill(
        SkillManifest(name="rt-str", version="0.0.1", tools=[ToolSpec(
            name="my_tool", description="t", parameters={}, capability="treatment.plan")]),
        {"my_tool": my_tool},
    )
    try:
        class _P:
            async def chat(self, messages, model=None, json_mode=False, tools=None):
                # T1.4 起 chat 返回 (content, usage)
                return "RAW", {}

        out, usage = await run_tool_loop(_P(), [{"role": "user", "content": "go"}],
                                         "m", "treatment.plan")
        assert out == "RAW"
    finally:
        skill_registry.unload("rt-str")


async def test_run_tool_loop_exhausts_rounds():
    # 工具轮次用尽后，做最后一次 json_mode 综合
    async def my_tool():
        return {}

    skill_registry.register_skill(
        SkillManifest(name="rt", version="0.0.1", tools=[ToolSpec(
            name="my_tool", description="t", parameters={}, capability="treatment.plan")]),
        {"my_tool": my_tool},
    )
    try:
        class _P:
            def __init__(self) -> None:
                self.n = 0

            async def chat(self, messages, model=None, json_mode=False, tools=None):
                # T1.4 起 chat 返回 (content, usage)
                self.n += 1
                if json_mode:
                    return "DONE", {}
                return {"content": "", "tool_calls": [{
                    "id": "c", "type": "function",
                    "function": {"name": "my_tool", "arguments": "{}"}}]}, {}

        out, usage = await run_tool_loop(_P(), [{"role": "user", "content": "go"}],
                                         "m", "treatment.plan", max_tool_rounds=2)
        assert out == "DONE"
    finally:
        skill_registry.unload("rt")


async def test_run_tool_loop_executes_real_tool():
    # 正常执行工具：覆盖工具执行主路径与参数解析成功分支
    captured: dict = {}

    async def my_tool(q: str = ""):
        captured["q"] = q
        return {"ok": True}

    skill_registry.register_skill(
        SkillManifest(name="rt-call", version="0.0.1", tools=[ToolSpec(
            name="my_tool", description="t",
            parameters={"type": "object", "properties": {"q": {"type": "string"}},
                        "required": ["q"]},
            capability="treatment.plan")]),
        {"my_tool": my_tool},
    )
    try:
        class _P:
            def __init__(self) -> None:
                self.n = 0

            async def chat(self, messages, model=None, json_mode=False, tools=None):
                # T1.4 起 chat 返回 (content, usage)
                self.n += 1
                if self.n == 1:
                    return {"content": "", "tool_calls": [{
                        "id": "c", "type": "function",
                        "function": {"name": "my_tool", "arguments": json.dumps({"q": "脾胃"})}}]}, {}
                return "DONE", {}

        out, usage = await run_tool_loop(_P(), [{"role": "user", "content": "go"}],
                                         "m", "treatment.plan")
        assert out == "DONE"
        assert captured["q"] == "脾胃"
    finally:
        skill_registry.unload("rt-call")


async def test_run_tool_loop_invalid_args_json():
    # arguments 不是合法 JSON -> 解析异常分支（args 退化为 {}）
    called: dict = {}

    async def my_tool(**kwargs):
        called["kw"] = kwargs
        return {}

    skill_registry.register_skill(
        SkillManifest(name="rt-badarg", version="0.0.1", tools=[ToolSpec(
            name="my_tool", description="t", parameters={}, capability="treatment.plan")]),
        {"my_tool": my_tool},
    )
    try:
        class _P:
            def __init__(self) -> None:
                self.n = 0

            async def chat(self, messages, model=None, json_mode=False, tools=None):
                # T1.4 起 chat 返回 (content, usage)
                self.n += 1
                if self.n == 1:
                    return {"content": "", "tool_calls": [{
                        "id": "c", "type": "function",
                        "function": {"name": "my_tool", "arguments": "{not json"}}]}, {}
                return "DONE", {}

        out, usage = await run_tool_loop(_P(), [{"role": "user", "content": "go"}],
                                         "m", "treatment.plan")
        assert out == "DONE"
        assert called["kw"] == {}
    finally:
        skill_registry.unload("rt-badarg")


async def test_run_tool_loop_tool_raises():
    # 工具执行抛异常 -> 异常分支（result 变为 {"error": ...}），不击穿流程
    async def my_tool():
        raise ValueError("boom")

    skill_registry.register_skill(
        SkillManifest(name="rt-err", version="0.0.1", tools=[ToolSpec(
            name="my_tool", description="t", parameters={}, capability="treatment.plan")]),
        {"my_tool": my_tool},
    )
    try:
        class _P:
            def __init__(self) -> None:
                self.n = 0

            async def chat(self, messages, model=None, json_mode=False, tools=None):
                # T1.4 起 chat 返回 (content, usage)
                self.n += 1
                if self.n == 1:
                    return {"content": "", "tool_calls": [{
                        "id": "c", "type": "function",
                        "function": {"name": "my_tool", "arguments": "{}"}}]}, {}
                return "DONE", {}

        out, usage = await run_tool_loop(_P(), [{"role": "user", "content": "go"}],
                                         "m", "treatment.plan")
        assert out == "DONE"
    finally:
        skill_registry.unload("rt-err")
