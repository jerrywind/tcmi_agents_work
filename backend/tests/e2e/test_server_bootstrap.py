"""E2E：真实服务启动、健康检查、OpenAPI、静态文档与 CORS。"""
from __future__ import annotations

import httpx
import pytest

pytestmark = pytest.mark.e2e


def test_health(server_url: str):
    r = httpx.get(f"{server_url}/api/health", timeout=5)
    assert r.status_code == 200
    assert r.json()["ok"] is True


def test_openapi_and_docs(client: httpx.Client):
    r = client.get("/openapi.json")
    assert r.status_code == 200
    paths = r.json()["paths"]
    assert "/api/consultations" in paths
    assert "/api/skills" in paths

    d = client.get("/docs")
    assert d.status_code == 200
    assert "swagger" in d.text.lower()


def test_cors_actual_request(client: httpx.Client):
    r = client.get("/api/health", headers={"Origin": "http://example.com"})
    assert r.status_code == 200
    assert r.headers.get("access-control-allow-origin") is not None


def test_cors_preflight(client: httpx.Client):
    r = client.options(
        "/api/consultations",
        headers={
            "Origin": "http://example.com",
            "Access-Control-Request-Method": "POST",
            "Access-Control-Request-Headers": "content-type",
        },
    )
    assert r.status_code in (200, 204)
    assert "access-control-allow-methods" in r.headers
