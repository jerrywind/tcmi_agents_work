"""全链路端到端测试公共配置。

覆盖组件：
  - frontend  (Taro H5，src/services/api.ts 提供的函数式 service 层)
  - backend   (FastAPI, 默认 :8000, /api/*)
  - rrserver  (Rust 反向隧道, server:8088 + client:9000)
  - llm_server(LM Studio 网关 + Agent, 默认 :8000, /healthz, /v1/*)

本 conftest 负责：
  1. 从环境变量读取各组件地址（带默认值）；
  2. 提供 wait_for_health 工具，等待服务就绪（带超时，符合 10 分钟硬性上限精神）；
  3. 暴露 httpx 客户端 fixture 与若干辅助函数。

注意：真实 LLM 推理依赖宿主机 LM Studio。在无 LM Studio 时，backend 的
get_provider() 会在 TCM_LLM_BASE_URL 留空时自动回退 MockProvider（规则兜底），
从而把整条问诊链路驱动到 finished，让全链路在没有 GPU/模型的情况下也能跑通。
llm_server 在无上游时会诚实返回 degraded（/healthz）与 503（/v1/models）。
"""
from __future__ import annotations

import os
import time

import httpx
import pytest

# ---------- 各组件地址（可用环境变量覆盖） ----------
BACKEND_BASE = os.getenv("TCM_BACKEND_BASE", "http://localhost:8000")
LLM_BASE = os.getenv("TCM_LLM_BASE", "http://localhost:8000")
RRSERVER_SERVER_BASE = os.getenv("TCM_RRSERVER_SERVER_BASE", "http://localhost:8088")
RRSERVER_CLIENT_BASE = os.getenv("TCM_RRSERVER_CLIENT_BASE", "http://localhost:9000")
FRONTEND_BASE = os.getenv("TCM_FRONTEND_BASE", "http://localhost:10086")

# 健康检查超时（秒）。保持远小于 10 分钟硬性上限。
HEALTH_TIMEOUT = float(os.getenv("TCM_E2E_HEALTH_TIMEOUT", "60"))
HTTP_TIMEOUT = float(os.getenv("TCM_E2E_HTTP_TIMEOUT", "30"))


def wait_for_health(url: str, *, timeout: float = HEALTH_TIMEOUT,
                    ok_predicate=None, label: str = "service") -> dict:
    """轮询一个健康端点直到就绪或超时。

    ok_predicate(resp_json) -> bool 用于判断“业务上就绪”。
    返回最后一次成功的响应的 JSON。
    """
    deadline = time.time() + timeout
    last_err: Exception | None = None
    while time.time() < deadline:
        try:
            with httpx.Client(timeout=5) as c:
                r = c.get(url)
                if r.status_code < 500:
                    try:
                        data = r.json()
                    except Exception:
                        data = {}
                    if ok_predicate is None or ok_predicate(data):
                        return data
                    last_err = RuntimeError(f"{label} 未就绪: {data}")
        except Exception as e:  # noqa: BLE001
            last_err = e
        time.sleep(1.0)
    raise TimeoutError(f"{label} 在 {timeout}s 内未就绪: {last_err}")


@pytest.fixture(scope="session")
def backend_base() -> str:
    return BACKEND_BASE


@pytest.fixture(scope="session")
def llm_base() -> str:
    return LLM_BASE


@pytest.fixture(scope="session")
def rrserver_server_base() -> str:
    return RRSERVER_SERVER_BASE


@pytest.fixture(scope="session")
def rrserver_client_base() -> str:
    return RRSERVER_CLIENT_BASE


@pytest.fixture(scope="session")
def frontend_base() -> str:
    return FRONTEND_BASE


@pytest.fixture(scope="session")
def backend_client(backend_base: str) -> httpx.Client:
    """backend 的同步 httpx 客户端。"""
    with httpx.Client(base_url=backend_base, timeout=HTTP_TIMEOUT) as c:
        yield c


@pytest.fixture(scope="session")
def llm_client(llm_base: str) -> httpx.Client:
    with httpx.Client(base_url=llm_base, timeout=HTTP_TIMEOUT) as c:
        yield c


@pytest.fixture(scope="session")
def rrserver_server_client(rrserver_server_base: str) -> httpx.Client:
    with httpx.Client(base_url=rrserver_server_base, timeout=HTTP_TIMEOUT) as c:
        yield c


@pytest.fixture(scope="session")
def rrserver_client_client(rrserver_client_base: str) -> httpx.Client:
    with httpx.Client(base_url=rrserver_client_base, timeout=HTTP_TIMEOUT) as c:
        yield c
