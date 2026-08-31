"""`app/rrclient.py` 单元测试：用假的 rrserver 验证注册 / 心跳 / 注销链路。

假 rrserver 是进程内的 ASGI 应用（经 `httpx.ASGITransport` 注入），不占端口、不联网。
契约与 `server/rrserver` 严格一致：

- `POST /api/register`  → `{name, ws_url, hash_code, transport, heartbeat_interval_millis, heartbeat_path}`
- `POST /api/heartbeat` → 请求 `{name, hash}`；200 或 404（注册已被回收）
- `POST /api/unregister`→ 请求 `{name, hash}`；200 或 404
"""
from __future__ import annotations

import asyncio

import httpx
from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse

from app.config import Settings
from app.rrclient import Registrar

# 测试里把心跳周期压到毫秒级；rr_retry_interval 也压到 1 秒
_INTERVAL_MS = 60


def make_fake_rrserver(
    *, interval_ms: int = _INTERVAL_MS, incomplete: bool = False
) -> tuple[FastAPI, dict]:
    """返回 (ASGI 应用, 内部状态字典)。

    状态里 `hash` 是当前有效的注册 hash，测试可直接改它来模拟「注册被云端回收」。
    """
    state: dict = {"seq": 0, "hash": None, "reg": [], "hb": [], "unreg": []}
    app = FastAPI()

    @app.post("/api/register")
    async def register(req: Request) -> dict:
        body = await req.json()
        state["reg"].append(body)
        if incomplete:  # 模拟不完整的注册响应（缺 hash / 周期）
            return {"name": body["name"], "ws_url": "ws://rr.test/ws/home"}
        state["seq"] += 1
        state["hash"] = f"hash{state['seq']:014d}"
        return {
            "name": body["name"],
            "ws_url": f"ws://rr.test/ws/{body['name']}",
            "hash_code": state["hash"],
            "transport": "http" if body.get("endpoint") else "ws",
            "heartbeat_interval_millis": interval_ms,
            "heartbeat_path": "/rr/heartbeat",
        }

    @app.post("/api/heartbeat")
    async def heartbeat(req: Request):
        body = await req.json()
        # 契约：心跳必须带 name，且与注册名一致
        if not body.get("name") or body.get("hash") != state["hash"]:
            return JSONResponse({"error": "unknown registration"}, status_code=404)
        state["hb"].append(body)
        return {
            "status": "ok",
            "name": body["name"],
            "hash": body["hash"],
            "heartbeat_interval_millis": interval_ms,
        }

    @app.post("/api/unregister")
    async def unregister(req: Request):
        body = await req.json()
        if not body.get("name") or body.get("hash") != state["hash"]:
            return JSONResponse({"error": "unknown registration"}, status_code=404)
        state["unreg"].append(body)
        return {"status": "removed", "name": body["name"], "hash": body["hash"]}

    return app, state


def _registrar(app: FastAPI, **overrides) -> Registrar:
    cfg = Settings()
    cfg.rr_server_base = "http://rr.test"
    cfg.rr_service_name = "home"
    cfg.rr_service_token = "secret"
    cfg.rr_service_endpoint = "http://llm_server:8000"
    cfg.rr_timeout = 5
    cfg.rr_retry_interval = 1
    for key, value in overrides.items():
        setattr(cfg, key, value)
    return Registrar(cfg, transport=httpx.ASGITransport(app=app))


async def test_register_obtains_hash_code_and_interval():
    app, state = make_fake_rrserver()
    r = _registrar(app)
    assert r.enabled

    assert await r.register() is True
    assert r.info.hash_code == "hash00000000000001"
    assert r.info.transport == "http", "带 endpoint 注册应为 http 直连形态"
    assert r.info.heartbeat_interval == 0.06

    # 请求体只带 name/token/endpoint：心跳周期由云端统一下发
    payload = state["reg"][0]
    assert payload["name"] == "home"
    assert payload["token"] == "secret"
    assert payload["endpoint"] == "http://llm_server:8000"
    assert "heartbeat_interval_secs" not in payload

    st = r.status()
    assert st["registered"] is True
    assert st["hash"] == r.info.hash_code
    assert st["heartbeat_interval_millis"] == _INTERVAL_MS
    assert st["heartbeat_age_secs"] == 0


async def test_register_rejects_incomplete_response():
    app, _ = make_fake_rrserver(incomplete=True)
    r = _registrar(app)

    assert await r.register() is False
    assert r.info.failures == 1
    assert "incomplete" in (r.info.last_error or "")
    assert r.status()["registered"] is False


async def test_heartbeat_loop_sends_heartbeats():
    app, state = make_fake_rrserver()
    r = _registrar(app)
    await r.start()
    try:
        await asyncio.sleep(0.4)
        assert len(state["hb"]) >= 2, f"心跳应持续上报，实际 {len(state['hb'])} 次"
        assert all(hb["name"] == "home" for hb in state["hb"])
        assert r.status()["detail"] == "ok"
    finally:
        await r.stop()


async def test_registration_expiry_triggers_automatic_reregister():
    # 云端回收注册（心跳 404）→ 客户端应立即重新注册，拿到**新的** hash code
    app, state = make_fake_rrserver()
    r = _registrar(app)
    await r.start()
    try:
        await asyncio.sleep(0.2)
        first_hash = r.info.hash_code
        assert first_hash

        state["hash"] = "0" * 16  # 模拟注册被回收
        deadline = asyncio.get_event_loop().time() + 3
        while asyncio.get_event_loop().time() < deadline:
            if r.info.hash_code and r.info.hash_code != first_hash:
                break
            await asyncio.sleep(0.05)

        assert r.info.hash_code != first_hash, "心跳 404 后应自动重新注册"
        assert r.status()["registered"] is True
        assert len(state["reg"]) >= 2, "重新注册应真的发起了第二次 register"
        assert state["unreg"] == [], "停止前不应注销"
    finally:
        await r.stop()


async def test_stop_unregisters_and_clears_state():
    app, state = make_fake_rrserver()
    r = _registrar(app)
    await r.start()
    await asyncio.sleep(0.1)
    current = r.info.hash_code

    await r.stop()
    assert state["unreg"] == [{"name": "home", "hash": current}]
    st = r.status()
    assert st["registered"] is False
    assert st["detail"] == "unregistered"
    assert st["hash"] == ""


async def test_disabled_without_server_base():
    cfg = Settings()
    cfg.rr_server_base = ""
    r = Registrar(cfg)
    assert r.enabled is False

    await r.start()
    await r.stop()
    st = r.status()
    assert st["enabled"] is False
    assert st["registered"] is False
    assert st["detail"] == "not registered"
