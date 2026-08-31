"""rrserver 反向隧道全链路 e2e。

验证「云端中继 server ↔ 家庭端 client ↔ 本地 llm stub」三段：
  1. server /healthz 返回 ok；
  2. server /api/register 对 token 做鉴权（错误 token 被拒），并为每次注册签发
     独立的 hash code（同名重注册换新 hash）；
  3. client 注册后，外部经 /t/<name>/* 发往 server 的请求，
     经 WS 隧道被 client 转发到本地 stub llm，并把响应原路回传；
  4. 心跳维护：`POST /api/heartbeat` 用 hash 报活（`GET /api/services` 可见刷新），
     未知 hash 返回 404；`POST /api/unregister` 关闭注册后该 hash 立即失效，
     client 随即自动重连并重新注册，隧道恢复。

本测试需要本地已编译的 rrserver 二进制（target/release 或 target/debug）。
未编译时整文件跳过，并给出构建提示，避免 CI 在无 Rust 环境硬失败。

同时需要一个「本地 llm stub」来充当隧道转发目标。这里用 Python 标准库
起一个临时 HTTP 服务，模拟 OpenAI 兼容的 /v1/models、/v1/chat/completions，
从而无需真实 LM Studio 即可验证隧道转发能力。
"""
from __future__ import annotations

import http.server
import json
import os
import re
import shutil
import socket
import subprocess
import threading
import time

import httpx
import pytest

# 候选二进制位置（debug 优先，构建更快）
#
# 路径随 rrserver 迁入 server/ 工作区而修正；此前仍指向迁移前的
# tcm_work/rrserver，导致即使已构建也一律跳过（静默「假绿」）。
# 后端本身只在 Docker 内构建，故这里额外支持 TCM_RRSERVER_BIN 显式指定。
_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RR_CANDIDATES = [
    os.environ.get("TCM_RRSERVER_BIN", ""),
    os.path.join(_ROOT, "server", "rrserver", "target", "debug", "rrserver.exe"),
    os.path.join(_ROOT, "server", "rrserver", "target", "release", "rrserver.exe"),
    os.path.join(_ROOT, "server", "rrserver", "target", "debug", "rrserver"),
    os.path.join(_ROOT, "server", "rrserver", "target", "release", "rrserver"),
]

TUNNEL_NAME = "home"
TUNNEL_TOKEN = "e2e-secret"


def _find_rrserver() -> str | None:
    for p in RR_CANDIDATES:
        if os.path.isfile(p):
            return p
    return None


class _StubLLM(http.server.BaseHTTPRequestHandler):
    """极简 OpenAI 兼容 stub，充当隧道转发目标。"""

    def log_message(self, *a):  # 静默
        pass

    def _json(self, payload: dict, status: int = 200):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):  # noqa: N802
        if self.path.rstrip("/").endswith("/v1/models"):
            self._json({"object": "list", "data": [
                {"id": "stub-model", "object": "model"}]})
        else:
            self._json({"echo": self.path, "method": "GET"})

    def do_POST(self):  # noqa: N802
        length = int(self.headers.get("Content-Length", "0") or "0")
        raw = self.rfile.read(length) if length else b"{}"
        try:
            req = json.loads(raw or b"{}")
        except Exception:
            req = {}
        if self.path.rstrip("/").endswith("/v1/chat/completions"):
            self._json({
                "id": "chatcmpl-stub",
                "object": "chat.completion",
                "model": req.get("model", "stub-model"),
                "choices": [{"index": 0, "message": {
                    "role": "assistant", "content": "stub reply"
                }, "finish_reason": "stop"}],
                "usage": {"total_tokens": 3},
            })
        else:
            self._json({"echo": self.path, "body": req, "method": "POST"})


@pytest.fixture(scope="module")
def stub_llm():
    """启动本地 stub llm 服务，返回其 base url。"""
    srv = http.server.HTTPServer(("127.0.0.1", 0), _StubLLM)
    port = srv.server_address[1]
    t = threading.Thread(target=srv.serve_forever, daemon=True)
    t.start()
    yield f"http://127.0.0.1:{port}"
    srv.shutdown()


def _free_port() -> int:
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


@pytest.fixture(scope="module")
def rrserver_pair(stub_llm):
    """启动 rrserver server + client，client 把隧道指向 stub_llm。

    返回 (server_base_url,)。server 监听随机空闲端口；
    client 经 /api/register 注册到该 server 并连 WS。
    """
    exe = _find_rrserver()
    if exe is None:
        pytest.skip(
            "未找到 rrserver 二进制，跳过隧道 e2e。构建方式：\n"
            "  cd rrserver && cargo build  (debug) 或 cargo build --release"
        )

    server_port = _free_port()
    server_url = f"http://127.0.0.1:{server_port}"
    ws_url = f"ws://127.0.0.1:{server_port}/ws/{TUNNEL_NAME}"

    # server 侧需要 config（含 [[tunnels]] 与 external_ws_base）。
    import tempfile
    cfg = tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False)
    # [health] 用极短心跳周期：让 client 的心跳在测试尺度内真实跑起来
    # （静默阈值保持默认 40 分钟，避免回收任务在测试期间把 client 的注册注销掉）
    cfg.write(f"""
external_ws_base = "ws://127.0.0.1:{server_port}"
[[tunnels]]
name = "{TUNNEL_NAME}"
token = "{TUNNEL_TOKEN}"
[health]
heartbeat_interval_secs = 1
reaper_interval_secs = 1
""")
    cfg.close()

    procs = []
    try:
        # 启动 server
        p_srv = subprocess.Popen(
            [exe, "server", "--listen", f"127.0.0.1:{server_port}",
             "--config", cfg.name],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        procs.append(p_srv)

        # 等待 server /healthz
        deadline = time.time() + 30
        ok = False
        while time.time() < deadline:
            try:
                with httpx.Client(timeout=2) as c:
                    if c.get(f"{server_url}/healthz").status_code == 200:
                        ok = True
                        break
            except Exception:
                pass
            time.sleep(0.5)
        if not ok:
            raise RuntimeError("rrserver server 未就绪")

        # 启动 client（家庭端隧道，转发到 stub_llm）
        p_cli = subprocess.Popen(
            [exe, "client", "--server", server_url,
             "--name", TUNNEL_NAME, "--token", TUNNEL_TOKEN,
             "--local", stub_llm],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        procs.append(p_cli)

        # 等待隧道注册：轮询 /t/<name>/v1/models 经隧道到达 stub
        deadline = time.time() + 30
        while time.time() < deadline:
            try:
                with httpx.Client(timeout=3) as c:
                    r = c.get(f"{server_url}/t/{TUNNEL_NAME}/v1/models")
                    if r.status_code == 200 and "stub-model" in r.text:
                        break
            except Exception:
                pass
            time.sleep(0.5)

        yield server_url
    finally:
        for p in procs:
            try:
                p.terminate()
                p.wait(timeout=5)
            except Exception:
                p.kill()
        try:
            os.unlink(cfg.name)
        except Exception:
            pass


def test_rrserver_healthz(rrserver_pair):
    base = rrserver_pair
    with httpx.Client(timeout=10) as c:
        r = c.get(f"{base}/healthz")
        assert r.status_code == 200
        assert r.text.strip() == "ok"


def test_rrserver_register_auth(rrserver_pair):
    """错误 token 应被拒绝，正确 token 应返回 ws_url。"""
    base = rrserver_pair
    with httpx.Client(timeout=10) as c:
        bad = c.post(f"{base}/api/register",
                     json={"name": TUNNEL_NAME, "token": "wrong"})
        assert bad.status_code in (401, 403)

        good = c.post(f"{base}/api/register",
                      json={"name": TUNNEL_NAME, "token": TUNNEL_TOKEN})
        assert good.status_code == 200
        assert "ws_url" in good.json()


def test_rrserver_tunnel_forwards_to_local_llm(rrserver_pair):
    """经 /t/<name>/v1/chat/completions 的请求应被隧道转发到本地 stub 并返回。"""
    base = rrserver_pair
    with httpx.Client(timeout=30) as c:
        r = c.post(
            f"{base}/t/{TUNNEL_NAME}/v1/chat/completions",
            json={"model": "stub-model", "messages": [{"role": "user", "content": "hi"}]},
        )
        assert r.status_code == 200, r.text
        body = r.json()
        assert body["choices"][0]["message"]["content"] == "stub reply"
        assert body["model"] == "stub-model"


def test_rrserver_register_issues_hash_code(rrserver_pair):
    """注册应签发独立 hash code：同一 name 每次注册都能拿到新的 hash。"""
    base = rrserver_pair
    with httpx.Client(timeout=10) as c:
        first = c.post(f"{base}/api/register",
                       json={"name": TUNNEL_NAME, "token": TUNNEL_TOKEN}).json()
        second = c.post(f"{base}/api/register",
                        json={"name": TUNNEL_NAME, "token": TUNNEL_TOKEN}).json()

    for payload in (first, second):
        assert re.fullmatch(r"[0-9a-f]{16}", payload.get("hash_code", "")), payload
        assert payload["transport"] == "ws"
        # fixture 里配的 1s 心跳周期应随注册响应下发（毫秒）
        assert payload["heartbeat_interval_millis"] == 1000
        assert "heartbeat_interval_secs" not in payload
    assert first["hash_code"] != second["hash_code"], "每次注册应签发新的 hash code"


def test_rrserver_heartbeat_and_services_listing(rrserver_pair):
    """心跳上报：正确 hash 被接受，未知 hash 返回 404（提示需重新注册）。"""
    base = rrserver_pair
    with httpx.Client(timeout=10) as c:
        reg = c.post(f"{base}/api/register",
                     json={"name": TUNNEL_NAME, "token": TUNNEL_TOKEN}).json()
        hb = c.post(f"{base}/api/heartbeat",
                    json={"name": TUNNEL_NAME, "hash": reg["hash_code"]})
        assert hb.status_code == 200, hb.text
        assert hb.json()["status"] == "ok"

        # 注册维护总览里应能看到这条注册，且心跳时间戳被刷新
        listing = c.get(f"{base}/api/services").json()
        entry = next((s for s in listing["services"]
                      if s["hash"] == reg["hash_code"]), None)
        assert entry is not None, listing
        assert entry["stale"] is False
        assert entry["heartbeat_age_secs"] <= 5

        unknown = c.post(f"{base}/api/heartbeat", json={"hash": "0" * 16})
        assert unknown.status_code == 404


def test_rrserver_unregister_closes_registration_then_client_reconnects(rrserver_pair):
    """注销后该 hash 立即失效；client 随后应自动重连并重新注册（隧道恢复）。

    必须放在文件最后：它会短暂中断同名隧道。
    """
    base = rrserver_pair
    with httpx.Client(timeout=10) as c:
        listing = c.get(f"{base}/api/services").json()
        current = next((s for s in listing["services"] if s["name"] == TUNNEL_NAME), None)
        assert current is not None, "client 应已注册"

        unreg = c.post(f"{base}/api/unregister",
                       json={"name": TUNNEL_NAME, "hash": current["hash"]})
        assert unreg.status_code == 200, unreg.text

        # 该 hash 立即失效：再用它心跳会收到 404
        assert c.post(f"{base}/api/heartbeat",
                      json={"name": TUNNEL_NAME, "hash": current["hash"]}).status_code == 404

    # client 侧：心跳 404（或隧道被关）→ 重连并重新注册，隧道恢复
    deadline = time.time() + 60
    recovered = False
    while time.time() < deadline:
        try:
            with httpx.Client(timeout=3) as c:
                r = c.get(f"{base}/t/{TUNNEL_NAME}/v1/models")
                if r.status_code == 200 and "stub-model" in r.text:
                    recovered = True
                    break
        except Exception:
            pass
        time.sleep(1)
    assert recovered, "注销注册后 client 应自动重连恢复隧道"
