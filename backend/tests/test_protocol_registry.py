"""协议与注册表测试：能力枚举、请求/响应信封、agent 解析与切换。"""
import pytest

import app.agents  # noqa: F401  确保各 sub-agent 已注册
from app import config
from app.protocol.base import (
    AgentRequest, AgentResponse, Capability, SubAgent,
)
from app.protocol.registry import (
    _REGISTRY, available_impls, build_request, register, resolve,
)
from app.protocol.llm import MockProvider, get_provider, image_content, parse_json


def test_capabilities_exist():
    names = {c.value for c in Capability}
    expected = {
        "diagnosis.inspection", "diagnosis.listening", "diagnosis.inquiry",
        "diagnosis.palpation", "diagnosis.differentiation", "diagnosis.safety",
        "treatment.plan",
    }
    assert expected <= names


def test_build_request_envelope():
    req = build_request(
        Capability.TREATMENT, session_id="s1", round=2,
        payload={"a": 1}, evidences=[], hypotheses=[], asked_keys=[],
    )
    assert isinstance(req, AgentRequest)
    assert req.session_id == "s1"
    assert req.round == 2
    assert req.capability == Capability.TREATMENT
    assert req.model  # 由 routing 解析得到
    assert req.options  # treatment.plan 携带 max_questions


def test_registry_resolves_rule_by_default():
    agent, route = resolve(Capability.TREATMENT)
    assert route["impl"] == "rule"
    assert agent.impl_name == "rule"


def test_available_impls():
    # mcp = 经 MCP 协议路由到远程实现（见 app/mcp/remote_agent.py）
    impls = available_impls(Capability.DIFFERENTIATION)
    assert {"rule", "llm"} <= set(impls)
    assert "mcp" in impls


def test_resolve_switch_to_llm():
    original = config.settings.routing["treatment.plan"]["impl"]
    config.settings.routing["treatment.plan"]["impl"] = "llm"
    try:
        agent, route = resolve(Capability.TREATMENT)
        assert route["impl"] == "llm"
        assert agent.impl_name == "llm"
    finally:
        config.settings.routing["treatment.plan"]["impl"] = original


def test_resolve_returns_instance():
    agent, _ = resolve(Capability.SAFETY)
    assert isinstance(agent, SubAgent)


async def test_mock_provider_and_parsers(tmp_path):
    p = get_provider()
    assert isinstance(p, MockProvider)
    text, usage = await p.chat([{"role": "user", "content": "hi"}], model="m")
    assert text == ""
    assert usage == {}
    assert parse_json("not json {") == {}
    assert parse_json('{"a": 1}') == {"a": 1}
    # image_content 会真实读取本地文件字节
    f = tmp_path / "img.jpg"
    f.write_bytes(b"binary")
    assert isinstance(image_content(str(f)), dict)


def test_subagent_registry_populated():
    assert len(_REGISTRY) >= 7


def test_register_decorator():
    # 验证 register 装饰器能把子类登记进 _REGISTRY（使用唯一 impl 名避免冲突）
    class _TmpAgent(SubAgent):
        capability = Capability.INSPECTION
        impl_name = "tmp"

        def handle(self, req):  # pragma: no cover - 仅测试注册
            return AgentResponse(capability=self.capability)

    decorated = register(_TmpAgent)
    assert decorated is _TmpAgent
    assert (Capability.INSPECTION.value, "tmp") in _REGISTRY


def test_register_duplicate_raises():
    class _A(SubAgent):
        capability = Capability.PALPATION
        impl_name = "dup-test"

        def handle(self, req):  # pragma: no cover - 仅测试注册
            return AgentResponse(capability=self.capability)

    register(_A)

    class _B(SubAgent):
        capability = Capability.PALPATION
        impl_name = "dup-test"

        def handle(self, req):  # pragma: no cover - 仅测试注册
            return AgentResponse(capability=self.capability)

    with pytest.raises(ValueError):
        register(_B)


def test_resolve_falls_back_to_rule_when_impl_missing():
    original = config.settings.routing["treatment.plan"]["impl"]
    config.settings.routing["treatment.plan"]["impl"] = "does-not-exist"
    try:
        agent, _ = resolve(Capability.TREATMENT)
        assert agent.impl_name == "rule"  # 兜底到 rule
    finally:
        config.settings.routing["treatment.plan"]["impl"] = original


def test_resolve_raises_when_no_impl_and_no_rule():
    cap = Capability.PALPATION
    rule_key = (cap.value, "rule")
    saved = _REGISTRY.pop(rule_key, None)
    original = config.settings.routing["diagnosis.palpation"]["impl"]
    config.settings.routing["diagnosis.palpation"]["impl"] = "does-not-exist"
    try:
        with pytest.raises(KeyError):
            resolve(cap)
    finally:
        if saved is not None:
            _REGISTRY[rule_key] = saved
        config.settings.routing["diagnosis.palpation"]["impl"] = original
