"""MCP Server 测试：两层工具粒度、Agent 级能力工具、错误处理。"""
from __future__ import annotations

import json

import pytest

import app.agents  # noqa: F401  触发 sub-agent 注册
from app.config import settings
from app.mcp import server as mcp_server
from app.mcp.tools import agents as agent_tools
from app.protocol.base import Capability


def _payload(contents):
    """把 MCP TextContent 列表解析回 Python 对象。"""
    assert contents, "工具应返回内容"
    return json.loads(contents[0].text)


class TestToolListing:
    def test_exposes_both_layers(self):
        names = {t.name for t in mcp_server.list_tools()}
        # 会话级
        assert {"create_consultation", "start_consultation", "answer_question",
                "get_state", "get_report"} <= names
        # Agent 级
        assert {"agent_inspection", "agent_listening", "agent_inquiry",
                "agent_palpation", "agent_differentiation", "agent_treatment",
                "agent_safety", "run_agent", "list_agent_capabilities"} <= names

    def test_every_capability_has_a_tool(self):
        names = {t.name for t in agent_tools.list_tools()}
        for cap in Capability:
            tool = agent_tools.CAPABILITY_TOOL[cap.value]
            assert tool in names, f"{cap.value} 缺少对应 MCP 工具"

    def test_tool_names_unique(self):
        names = [t.name for t in mcp_server.list_tools()]
        assert len(names) == len(set(names))

    def test_schemas_are_valid_json_schema(self):
        for t in mcp_server.list_tools():
            assert t.description, f"{t.name} 缺少描述"
            assert t.inputSchema.get("type") == "object", f"{t.name} schema 非法"
            assert isinstance(t.inputSchema.get("properties", {}), dict)

    def test_expose_flags_respected(self, monkeypatch):
        monkeypatch.setitem(settings.mcp["server"], "expose_session_tools", False)
        names = {t.name for t in mcp_server.list_tools()}
        assert "create_consultation" not in names
        assert "agent_safety" in names

        monkeypatch.setitem(settings.mcp["server"], "expose_agent_tools", False)
        assert mcp_server.list_tools() == []


class TestAgentTools:
    async def test_safety_detects_red_flag(self):
        out = _payload(await mcp_server.handle_call(
            "agent_safety", {"text": "突然剧烈胸痛并大汗淋漓"}))
        assert out["capability"] == Capability.SAFETY.value
        assert out["status"] == "ok"
        assert out["alerts"], "红旗症状应产生告警"

    async def test_listening_produces_evidences(self):
        out = _payload(await mcp_server.handle_call(
            "agent_listening", {"text": "咳嗽有痰，口臭明显"}))
        assert out["status"] == "ok"
        assert isinstance(out["evidences"], list)

    async def test_differentiation_produces_hypotheses(self, sample_evidences):
        evs = [e.model_dump(mode="json") for e in sample_evidences()]
        out = _payload(await mcp_server.handle_call(
            "agent_differentiation", {"evidences": evs}))
        assert out["status"] == "ok"
        assert out["hypotheses"], "足够证据应产出候选证候"

    async def test_inquiry_returns_question(self, sample_evidences):
        evs = [e.model_dump(mode="json") for e in sample_evidences()]
        out = _payload(await mcp_server.handle_call(
            "agent_inquiry", {"evidences": evs, "asked_keys": [], "gender": "女", "age": 30}))
        assert out["status"] == "ok"
        assert out["question"] is None or "id" in out["question"]

    async def test_is_stateless_across_calls(self):
        """Agent 级工具无状态：相同输入两次调用结果一致。"""
        args = {"text": "咳嗽有痰"}
        a = _payload(await mcp_server.handle_call("agent_listening", args))
        b = _payload(await mcp_server.handle_call("agent_listening", args))
        assert a["evidences"] == b["evidences"]

    async def test_payload_object_form_supported(self):
        """既支持扁平字段，也支持 payload 对象。"""
        flat = _payload(await mcp_server.handle_call("agent_safety", {"text": "剧烈胸痛"}))
        nested = _payload(await mcp_server.handle_call(
            "agent_safety", {"payload": {"text": "剧烈胸痛"}}))
        assert flat["alerts"] == nested["alerts"]


class TestRunAgentAndIntrospection:
    async def test_run_agent_generic_entry(self):
        out = _payload(await mcp_server.handle_call(
            "run_agent",
            {"capability": Capability.SAFETY.value, "payload": {"text": "剧烈胸痛伴大汗"}}))
        assert out["capability"] == Capability.SAFETY.value
        assert out["alerts"]

    async def test_run_agent_rejects_unknown_capability(self):
        out = _payload(await mcp_server.handle_call("run_agent", {"capability": "nope"}))
        assert "error" in out and "nope" in out["error"]

    async def test_list_capabilities_overview(self):
        out = _payload(await mcp_server.handle_call("list_agent_capabilities", {}))
        assert len(out) == len(Capability)
        caps = {i["capability"] for i in out}
        assert caps == {c.value for c in Capability}
        for item in out:
            assert item["tool"]
            assert "mcp" in item["available_impls"], "每个能力都应可远程化"

    async def test_unknown_tool_returns_error(self):
        out = _payload(await mcp_server.handle_call("no_such_tool", {}))
        assert "error" in out
        assert "unknown tool" in out["error"]


class TestServerConstruction:
    def test_build_server(self):
        s = mcp_server.build_server()
        assert s.name == mcp_server.SERVER_NAME

    def test_build_http_app(self):
        endpoint = mcp_server.build_http_app()
        assert callable(endpoint)
        assert hasattr(endpoint, "run")

    async def test_http_endpoint_is_reentrant(self):
        """同一挂载对象可反复进出 lifespan（支持 reload / 多次建测试客户端）。"""
        endpoint = mcp_server.build_http_app()
        for _ in range(2):
            async with endpoint.run():
                assert endpoint._manager is not None
            assert endpoint._manager is None

    async def test_returns_503_when_not_running(self):
        """未随 lifespan 启动时返回明确 503，而非抛 RuntimeError。"""
        endpoint = mcp_server.build_http_app()
        sent: list[dict] = []

        async def send(msg):
            sent.append(msg)

        async def receive():
            return {"type": "http.request", "body": b"", "more_body": False}

        await endpoint({"type": "http", "method": "GET", "path": "/mcp"}, receive, send)
        assert sent[0]["status"] == 503
