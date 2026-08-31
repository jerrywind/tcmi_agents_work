"""探测与注册状态相关的 HTTP 端点：`GET /rr/heartbeat`、`GET /healthz`。

`/rr/heartbeat` 是 rrserver 的探活端点（以 `transport=http` 注册时由云端调用）；
`/healthz` 中新增的 `rrserver` 字段展示注册状态，便于排查。
"""
from __future__ import annotations

from fastapi.testclient import TestClient

from app.main import app


def test_rr_heartbeat_reports_alive_and_registration_state():
    with TestClient(app) as client:
        resp = client.get("/rr/heartbeat")
    assert resp.status_code == 200
    body = resp.json()
    assert body["status"] == "ok"
    assert body["service"] == "llm_server"

    rr = body["rrserver"]
    assert rr["enabled"] is False, "未配置 RR_SERVER_BASE 时注册不启用"
    assert rr["registered"] is False
    assert rr["hash"] == ""
    # 周期字段与 rrserver 一致用毫秒
    assert rr["heartbeat_interval_millis"] == 1_800_000
    assert rr["heartbeat_age_secs"] is None


def test_healthz_includes_rrserver_registration_state():
    with TestClient(app) as client:
        resp = client.get("/healthz")
    assert resp.status_code == 200
    body = resp.json()
    assert body["service"] == "llm_server"
    assert set(body["rrserver"]) >= {"enabled", "registered", "detail"}
