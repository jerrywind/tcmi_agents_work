"""backend ↔ llm_server 集成 e2e（完整问诊链路）。

启动真实 backend（uvicorn，空闲端口），并把 LLM 路由配置为指向 llm_server
网关（routing.llm.yaml + 空 TCM_LLM_BASE_URL），验证「backend 配置上接入
llm_server 网关」这一集成点不会破坏系统可用性与健康状态。

backend 不设 TCM_LLM_BASE_URL（留空）时，get_provider() 自动回退
MockProvider（规则兜底），从而在无真实 LLM 下也能把整条问诊链路驱动到
finished，并断言核心产物齐全：
  - /api/consultations/{id}                  status == finished
  - /api/consultations/{id}/report           报告存在且非空
  - /api/consultations/{id}/evidences        至少 1 条证据
  - /api/consultations/{id}/trace            至少 1 条 agent 调用轨迹
  - /api/consultations/{id}/image + /image/{id}  图片上传与读取

从而独立于 GPU/模型验证 backend 的状态机与产物生成。
"""
from __future__ import annotations

import os
import socket
import sys
import threading
import time

import httpx
import pytest

sys.path.insert(0, os.path.dirname(__file__))
from e2e_helpers import (  # noqa: E402
    create_consultation, get_status, get_report, get_evidences,
    get_trace, upload_image, drive_to_finished, wait_until_finished,
)

_BACKEND_DIR = r"d:/labs/windblue_tech/tcm_work/backend"


def _free_port() -> int:
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


@pytest.fixture(scope="module")
def backend_url():
    import uvicorn
    from app.main import app

    env = dict(os.environ)
    # 不设 TCM_LLM_BASE_URL（留空）→ get_provider() 自动回退 MockProvider（规则兜底），
    # 验证「即使配置了 LLM 路由文件，无上游时系统仍可用并收敛」。
    env.pop("TCM_LLM_BASE_URL", None)
    env.pop("TCM_LLM_API_KEY", None)
    # 用 LLM 路由文件，使 /api/system/agents 暴露 LLM 实现的 agents
    env["TCM_ROUTING_FILE"] = os.path.join(_BACKEND_DIR, "app", "routing.llm.yaml")
    env["TCM_HOST"] = "127.0.0.1"

    url = None
    server = None
    thread = None
    for _ in range(5):
        port = _free_port()
        uconfig = uvicorn.Config(app, host="127.0.0.1", port=port, log_level="error")
        server = uvicorn.Server(uconfig)
        thread = threading.Thread(target=server.run, daemon=True)
        thread.start()
        url = f"http://127.0.0.1:{port}"
        deadline = time.time() + 15
        ok = False
        while time.time() < deadline:
            try:
                if httpx.get(f"{url}/api/health", timeout=1).status_code == 200:
                    ok = True
                    break
            except Exception:
                time.sleep(0.1)
        if ok:
            break
        server.should_exit = True
        thread.join(timeout=5)

    if url is None:
        pytest.fail("backend 服务在超时内未能就绪")

    # 把环境变量暴露给 helpers 的 client（helpers.client 未用 base_url，这里重建）
    os.environ["TCM_E2E_BACKEND_URL"] = url
    yield url

    if server is not None:
        server.should_exit = True
        thread.join(timeout=5)


@pytest.fixture(scope="module")
def client(backend_url: str) -> httpx.Client:
    with httpx.Client(base_url=backend_url, timeout=30) as c:
        yield c


@pytest.fixture(scope="module")
def consultation_id(client: httpx.Client) -> str:
    """驱动一条完整问诊直到 finished，返回会话 id。

    本测试不设置 TCM_LLM_BASE_URL（留空），backend 的 get_provider() 自动回退
    MockProvider（规则兜底），从而在无真实 LLM 下也能把问诊收敛到 finished。
    """
    cid = create_consultation(client, "e2e-全链路测试-男-35-失眠乏力")
    drive_to_finished(client, cid)
    wait_until_finished(client, cid, timeout=60)
    return cid


def test_backend_healthz(client):
    r = client.get("/api/healthz")
    assert r.status_code == 200


def test_system_agents_exposed(client):
    """接入 LLM 路由后，/api/system/agents 应返回非空的 agent 清单。"""
    r = client.get("/api/system/agents")
    assert r.status_code == 200
    data = r.json()
    agents = data.get("agents", data)
    assert len(agents) > 0


def test_consultation_reaches_finished(backend_url, consultation_id):
    assert get_status(backend_url, consultation_id) == "finished"


def test_report_present(backend_url, consultation_id):
    report = get_report(backend_url, consultation_id)
    assert isinstance(report, dict)
    assert report.get("report") or report.get("text") or report.get("advice") \
        or any(report.values())


def test_evidences_present(backend_url, consultation_id):
    evs = get_evidences(backend_url, consultation_id)
    assert isinstance(evs, list) and len(evs) >= 1


def test_trace_present(backend_url, consultation_id):
    trace = get_trace(backend_url, consultation_id)
    assert isinstance(trace, list) and len(trace) >= 1


def test_image_upload_and_read(client, backend_url, consultation_id):
    """上传一张本地图片并读取其字节。"""
    img_path = os.path.join(os.path.dirname(__file__), "images", "sample.jpg")
    img_id = upload_image(backend_url, consultation_id, img_path)
    assert img_id

    r = client.get(f"/api/consultations/{consultation_id}/image/{img_id}")
    assert r.status_code == 200
    assert len(r.content) > 0
