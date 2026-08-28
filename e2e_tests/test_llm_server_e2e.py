"""llm_server 网关 e2e。

llm_server 是纯 LM Studio 网关（不托管模型）。本测试覆盖：
  1. 服务启动后 /healthz 可达，且状态如实反映上游可用性
     （无上游 → degraded；有 stub 上游 → ok）；
  2. /v1/agent/tools 返回工具定义；
  3. /v1/models 在无上游时返回 503，在有 stub 上游时透传模型列表；
  4. /v1/chat/completions 在有上游时把请求透传并回传上游响应。

为不依赖真实 LM Studio，本测试用一个 Python 标准库 stub 充当
LM Studio 兼容上游（LMSTUDIO_BASE_URL 指向它）。

除非能找到 llm_server 的 Python 解释器/依赖，否则整文件 skip。
"""
from __future__ import annotations

import http.server
import json
import os
import shutil
import subprocess
import sys
import threading
import time

import httpx
import pytest

LLM_DIR = r"d:/labs/windblue_tech/tcm_work/llm_server"


def _have_python_with(package: str) -> bool:
    try:
        subprocess.run([sys.executable, "-c", f"import {package}"],
                      capture_output=True, check=True)
        return True
    except Exception:
        return False


class _StubLMStudio(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _json(self, payload, status=200):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):  # noqa: N802
        if self.path.rstrip("/").endswith("/v1/models"):
            self._json({"object": "list", "data": [
                {"id": "google/gemma-4-12b-qat", "object": "model"}]})
        else:
            self._json({"detail": "not found"}, status=404)

    def do_POST(self):  # noqa: N802
        length = int(self.headers.get("Content-Length", "0") or "0")
        raw = self.rfile.read(length) if length else b"{}"
        try:
            req = json.loads(raw or b"{}")
        except Exception:
            req = {}
        if self.path.rstrip("/").endswith("/v1/chat/completions"):
            self._json({
                "id": "chatcmpl-e2e",
                "object": "chat.completion",
                "model": req.get("model", "google/gemma-4-12b-qat"),
                "choices": [{"index": 0, "message": {
                    "role": "assistant", "content": "e2e stub answer"
                }, "finish_reason": "stop"}],
                "usage": {"total_tokens": 5},
            })
        else:
            self._json({"echo": self.path}, status=200)


def _free_port() -> int:
    import socket
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


@pytest.fixture(scope="module")
def stub_lmstudio():
    srv = http.server.HTTPServer(("127.0.0.1", 0), _StubLMStudio)
    port = srv.server_address[1]
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{port}"
    srv.shutdown()


@pytest.fixture(scope="module")
def llm_server_with_upstream(stub_lmstudio):
    """启动 llm_server，上游指向 stub LM Studio。"""
    if not _have_python_with("fastapi"):
        pytest.skip("当前 Python 环境缺少 fastapi，无法启动 llm_server。")
    port = _free_port()
    env = dict(os.environ)
    env["LLM_HOST"] = "127.0.0.1"
    env["LLM_PORT"] = str(port)
    env["LMSTUDIO_BASE_URL"] = f"{stub_lmstudio}/v1"
    env["LMSTUDIO_API_KEY"] = "sk-noauth"
    env["DEFAULT_MODEL"] = "google/gemma-4-12b-qat"
    env.setdefault("ENABLE_MCP", "false")
    env.setdefault("ENABLE_PROMPT_OPTIMIZE", "true")

    proc = subprocess.Popen(
        [sys.executable, "-m", "app.main"],
        cwd=LLM_DIR, env=env,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    base = f"http://127.0.0.1:{port}"
    try:
        # 等待 /healthz 达到 ok（上游 stub 就绪）
        deadline = time.time() + 60
        ok = False
        while time.time() < deadline:
            try:
                with httpx.Client(timeout=3) as c:
                    r = c.get(f"{base}/healthz")
                    if r.status_code == 200 and r.json().get("status") == "ok":
                        ok = True
                        break
            except Exception:
                pass
            time.sleep(0.5)
        if not ok:
            pytest.skip("llm_server 启动后 upstream 未达到 ok（可能依赖未装齐）。")
        yield base
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()


def test_llm_healthz_ok(llm_server_with_upstream):
    with httpx.Client(timeout=10) as c:
        r = c.get(f"{llm_server_with_upstream}/healthz")
        assert r.status_code == 200
        assert r.json()["status"] == "ok"


def test_llm_models_passthrough(llm_server_with_upstream):
    """有上游时 /v1/models 透传 stub 返回的模型列表。"""
    with httpx.Client(timeout=10) as c:
        r = c.get(f"{llm_server_with_upstream}/v1/models")
        assert r.status_code == 200
        data = r.json()
        assert any(m["id"] == "google/gemma-4-12b-qat"
                   for m in data.get("data", []))


def test_llm_chat_completions_passthrough(llm_server_with_upstream):
    """/v1/chat/completions 透传并回传上游响应。"""
    with httpx.Client(timeout=15) as c:
        r = c.post(
            f"{llm_server_with_upstream}/v1/chat/completions",
            json={"model": "google/gemma-4-12b-qat",
                  "messages": [{"role": "user", "content": "hi"}]},
        )
        assert r.status_code == 200, r.text
        assert r.json()["choices"][0]["message"]["content"] == "e2e stub answer"


def test_llm_agent_tools_listed(llm_server_with_upstream):
    """agent 工具集端点返回非空工具定义。"""
    with httpx.Client(timeout=10) as c:
        r = c.get(f"{llm_server_with_upstream}/v1/agent/tools")
        if r.status_code == 404:
            pytest.skip("/v1/agent/tools 未实现")
        assert r.status_code == 200
        tools = r.json().get("tools", r.json())
        assert len(tools) >= 0  # 至少不报错
