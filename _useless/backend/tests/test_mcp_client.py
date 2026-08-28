"""MCP Client 测试：Hub 生命周期、工具注册/卸载、结果归一化、远程 Sub-Agent 桥。"""
from __future__ import annotations

import pytest

import app.agents  # noqa: F401
from app.config import settings
from app.mcp import remote_agent
from app.mcp.client import MCPConnectionError, MCPToolHub, _normalize_result
from app.protocol.base import AgentResponse, Capability
from app.protocol.registry import build_request, resolve
from app.skills.registry import skill_registry


class _Content:
    def __init__(self, text: str) -> None:
        self.type = "text"
        self.text = text


class _Result:
    def __init__(self, text: str) -> None:
        self.content = [_Content(text)]


class _FakeHub:
    """替身 Hub：记录调用并按预设返回，用于测试远程桥接而不起真实服务。"""

    def __init__(self, reply=None, connected=True, exc: Exception | None = None) -> None:
        self.reply = reply if reply is not None else {}
        self._connected = connected
        self.exc = exc
        self.calls: list[tuple[str, str, dict]] = []

    def is_connected(self, name: str) -> bool:
        return self._connected

    async def call(self, server: str, tool: str, args: dict):
        self.calls.append((server, tool, args))
        if self.exc:
            raise self.exc
        return self.reply


class TestNormalizeResult:
    def test_parses_json_text(self):
        assert _normalize_result(_Result('{"a": 1}')) == {"a": 1}

    def test_wraps_plain_text(self):
        assert _normalize_result(_Result("hello")) == {"result": "hello"}

    def test_empty_content(self):
        class _Empty:
            content = []
        assert _normalize_result(_Empty()) == {}


class TestHubLifecycle:
    async def test_initial_state(self):
        hub = MCPToolHub()
        assert hub.connected_servers == []
        assert hub.status() == []
        assert hub.is_connected("x") is False

    async def test_call_unconnected_raises(self):
        hub = MCPToolHub()
        with pytest.raises(MCPConnectionError):
            await hub.call("ghost", "tool", {})

    async def test_disconnect_unknown_returns_false(self):
        hub = MCPToolHub()
        assert await hub.disconnect("ghost") is False

    async def test_invalid_transport(self):
        hub = MCPToolHub()
        with pytest.raises(MCPConnectionError, match="失败"):
            await hub.connect("x", "carrier-pigeon")

    async def test_missing_url_for_http(self):
        hub = MCPToolHub()
        with pytest.raises(MCPConnectionError):
            await hub.connect("x", "http")

    async def test_missing_command_for_stdio(self):
        hub = MCPToolHub()
        with pytest.raises(MCPConnectionError):
            await hub.connect("x", "stdio")

    async def test_connect_from_config_records_failure(self, monkeypatch):
        """配置中的 Server 连不上时，只记录失败，不抛异常（不阻塞应用启动）。"""
        monkeypatch.setitem(settings.mcp, "clients", [
            {"name": "dead", "transport": "http", "url": "http://127.0.0.1:1/mcp",
             "enabled": True},
            {"name": "skipped", "transport": "http", "url": "http://x/mcp",
             "enabled": False},
        ])
        hub = MCPToolHub(call_timeout=1)
        report = await hub.connect_from_config()
        assert "dead" in report["failed"]
        assert "skipped" not in report["failed"]
        assert "skipped" not in report["connected"]

    async def test_close_is_idempotent(self):
        hub = MCPToolHub()
        await hub.close()
        await hub.close()


class TestSkillRegistration:
    """验证外部 MCP 工具能注册进 SKILL 体系并可被卸载。"""

    async def test_register_and_unload(self):
        hub = MCPToolHub()

        class _T:
            def __init__(self, name):
                self.name = name
                self.description = "d"
                self.inputSchema = {"type": "object", "properties": {}}

        class _Resp:
            tools = [_T("forecast"), _T("alerts")]

        class _Session:
            async def list_tools(self):
                return _Resp()

        names = await hub._register("weather", _Session())
        assert names == ["mcp__weather__forecast", "mcp__weather__alerts"]
        tool_names = {t.name for t in skill_registry.list_tools()}
        assert "mcp__weather__forecast" in tool_names
        # 对所有 capability 开放
        assert any(t["function"]["name"] == "mcp__weather__forecast"
                   for t in skill_registry.tools_for(Capability.INQUIRY.value))
        assert skill_registry.unload("mcp_weather") is True
        assert "mcp__weather__forecast" not in {t.name for t in skill_registry.list_tools()}


class TestRemoteSubAgent:
    """远程 Sub-Agent 桥：impl='mcp' 时把能力路由到外部 MCP Server。"""

    def test_all_capabilities_registered(self):
        from app.protocol.registry import available_impls
        for cap in Capability:
            assert "mcp" in available_impls(cap)

    def test_routing_switch_selects_mcp(self, monkeypatch):
        monkeypatch.setitem(settings.routing, Capability.SAFETY.value,
                            {"impl": "mcp", "options": {"server": "remote"}})
        agent, route = resolve(Capability.SAFETY)
        assert route["impl"] == "mcp"
        assert isinstance(agent, remote_agent.McpRemoteAgent)

    async def test_forwards_and_parses_response(self, monkeypatch):
        fake = _FakeHub(reply={
            "capability": Capability.SAFETY.value,
            "status": "ok",
            "alerts": [{"level": "danger", "reason": "胸痛", "advice": "急诊"}],
        })
        monkeypatch.setattr(remote_agent, "_get_hub", lambda: fake)
        agent = remote_agent.McpSafetyAgent()
        req = build_request(Capability.SAFETY, payload={"text": "胸痛"})
        req.options = {"server": "remote"}
        resp: AgentResponse = await agent.run(req)
        assert resp.status == "ok"
        assert resp.alerts[0].reason == "胸痛"
        assert resp.meta["remote_server"] == "remote"
        assert resp.meta["impl"] == "mcp"
        server, tool, args = fake.calls[0]
        assert (server, tool) == ("remote", "agent_safety")
        assert args["text"] == "胸痛"          # payload 铺平
        assert args["payload"] == {"text": "胸痛"}

    async def test_custom_tool_name(self, monkeypatch):
        fake = _FakeHub(reply={"capability": Capability.INQUIRY.value, "status": "ok"})
        monkeypatch.setattr(remote_agent, "_get_hub", lambda: fake)
        agent = remote_agent.McpInquiryAgent()
        req = build_request(Capability.INQUIRY)
        req.options = {"server": "r", "tool": "my_tool"}
        await agent.run(req)
        assert fake.calls[0][1] == "my_tool"

    async def test_run_agent_fallback_includes_capability(self, monkeypatch):
        """未按 capability 命名的远端用通用入口时应带上 capability 字段。"""
        fake = _FakeHub(reply={"capability": Capability.SAFETY.value, "status": "ok"})
        monkeypatch.setattr(remote_agent, "_get_hub", lambda: fake)
        agent = remote_agent.McpSafetyAgent()
        req = build_request(Capability.SAFETY)
        req.options = {"server": "r", "tool": "run_agent"}
        await agent.run(req)
        assert fake.calls[0][2]["capability"] == Capability.SAFETY.value

    async def test_missing_server_option_degrades(self, monkeypatch):
        monkeypatch.setattr(remote_agent, "_get_hub", lambda: _FakeHub())
        agent = remote_agent.McpSafetyAgent()
        req = build_request(Capability.SAFETY)
        req.options = {}
        resp = await agent.run(req)
        assert resp.status == "error"
        assert "options.server" in resp.error

    async def test_disconnected_server_degrades(self, monkeypatch):
        monkeypatch.setattr(remote_agent, "_get_hub", lambda: _FakeHub(connected=False))
        agent = remote_agent.McpSafetyAgent()
        req = build_request(Capability.SAFETY)
        req.options = {"server": "gone"}
        resp = await agent.run(req)
        assert resp.status == "error"
        assert "未连接" in resp.error

    async def test_remote_exception_degrades(self, monkeypatch):
        monkeypatch.setattr(remote_agent, "_get_hub",
                            lambda: _FakeHub(exc=TimeoutError("timeout")))
        agent = remote_agent.McpSafetyAgent()
        req = build_request(Capability.SAFETY)
        req.options = {"server": "slow"}
        resp = await agent.run(req)
        assert resp.status == "error"
        assert resp.capability == Capability.SAFETY

    async def test_remote_error_payload_degrades(self, monkeypatch):
        monkeypatch.setattr(remote_agent, "_get_hub",
                            lambda: _FakeHub(reply={"error": "boom"}))
        agent = remote_agent.McpSafetyAgent()
        req = build_request(Capability.SAFETY)
        req.options = {"server": "r"}
        resp = await agent.run(req)
        assert resp.status == "error"
        assert "boom" in resp.error
