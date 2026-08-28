"""MCP 管理端点与配置解析测试。"""
from __future__ import annotations

import json

import pytest
from fastapi.testclient import TestClient

from app.config import Settings, settings
from app.main import app
from app.protocol.base import Capability


@pytest.fixture
def client(monkeypatch):
    """走完整 lifespan（启动 MCP session manager），并清空自动连接配置。"""
    monkeypatch.setitem(settings.mcp, "clients", [])
    with TestClient(app) as c:
        yield c


class TestMcpStatusApi:
    def test_status_shape(self, client):
        r = client.get("/api/mcp/status")
        assert r.status_code == 200
        data = r.json()
        assert data["server"]["enabled"] is True
        assert data["server"]["mount_path"] == "/mcp"
        assert data["server"]["tool_count"] > 0
        assert data["clients"] == []
        assert len(data["capabilities"]) == len(Capability)

    def test_capabilities_report_mcp_impl(self, client):
        caps = client.get("/api/mcp/status").json()["capabilities"]
        for item in caps:
            assert "mcp" in item["available_impls"]
            assert item["tool"].startswith("agent_")

    def test_tools_listing(self, client):
        data = client.get("/api/mcp/tools").json()
        names = {t["name"] for t in data["tools"]}
        assert "agent_differentiation" in names
        assert "create_consultation" in names
        for t in data["tools"]:
            assert t["input_schema"]["type"] == "object"


class TestMcpClientApi:
    def test_connect_missing_params(self, client):
        r = client.post("/api/mcp/clients", json={"name": "x", "transport": "http"})
        assert r.status_code in (400, 422)

    def test_connect_unreachable(self, client):
        r = client.post("/api/mcp/clients", json={
            "name": "dead", "transport": "http", "url": "http://127.0.0.1:1/mcp"})
        assert r.status_code == 400

    def test_disconnect_unknown(self, client):
        r = client.delete("/api/mcp/clients/ghost")
        assert r.status_code == 404


class TestMcpMount:
    def test_mcp_endpoint_mounted(self, client):
        """/mcp 已挂载且 session manager 已随 lifespan 启动。

        未按 MCP 规范带 Accept 头时，协议层应返回 4xx（而非 404 未挂载、
        也非 503 未启动）。
        """
        r = client.get("/mcp")
        assert r.status_code != 404, "/mcp 未挂载"
        assert r.status_code != 503, "MCP session manager 未随 lifespan 启动"
        assert 400 <= r.status_code < 500

    def test_initialize_handshake(self, client):
        """完整 MCP initialize 握手应成功返回服务器信息。"""
        r = client.post(
            "/mcp",
            headers={"Accept": "application/json, text/event-stream",
                     "Content-Type": "application/json"},
            json={"jsonrpc": "2.0", "id": 1, "method": "initialize",
                  "params": {"protocolVersion": "2025-03-26",
                             "capabilities": {},
                             "clientInfo": {"name": "test", "version": "1.0"}}},
        )
        assert r.status_code == 200
        assert "tcm-consult" in r.text


class TestMcpConfig:
    def test_defaults(self):
        assert settings.mcp_server_enabled is True
        assert settings.mcp_mount_path == "/mcp"
        assert settings.mcp_call_timeout == 30

    def test_mount_path_normalized(self):
        merged = Settings._merge_mcp({"server": {"mount_path": "mcp2"}})
        s = object.__new__(Settings)
        s.mcp = merged
        assert Settings.mcp_mount_path.fget(s) == "/mcp2"

    def test_env_overrides(self, monkeypatch):
        monkeypatch.setenv("TCM_MCP_SERVER_ENABLED", "0")
        monkeypatch.setenv("TCM_MCP_MOUNT_PATH", "/x-mcp")
        monkeypatch.setenv("TCM_MCP_CALL_TIMEOUT", "5")
        monkeypatch.setenv("TCM_MCP_CLIENTS",
                           json.dumps([{"name": "a", "transport": "http", "url": "u"}]))
        merged = Settings._merge_mcp({})
        assert merged["server"]["enabled"] is False
        assert merged["server"]["mount_path"] == "/x-mcp"
        assert merged["call_timeout"] == 5
        assert merged["clients"][0]["name"] == "a"

    def test_bad_env_json_ignored(self, monkeypatch):
        monkeypatch.setenv("TCM_MCP_CLIENTS", "{not json")
        assert Settings._merge_mcp({})["clients"] == []

    def test_client_configs_filter_enabled(self, monkeypatch):
        monkeypatch.setitem(settings.mcp, "clients", [
            {"name": "on", "enabled": True},
            {"name": "off", "enabled": False},
            {"name": "default"},           # 未写 enabled 视为启用
            {"transport": "http"},         # 无 name，忽略
        ])
        names = [c["name"] for c in settings.mcp_client_configs()]
        assert names == ["on", "default"]
