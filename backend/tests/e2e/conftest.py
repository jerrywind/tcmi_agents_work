"""E2E 测试公共 fixtures：在独立线程中启动真实 uvicorn 服务，并用 httpx 直连。

与 ``test_main.py`` 的 ``TestClient`` 不同，这里走真实 socket / ASGI server /
CORS 中间件 / 静态文件挂载，验证的是「部署态」行为，而非仅路由逻辑。
服务默认运行在 pytest 主进程的一个守护线程中（与 pytest 同进程），因此覆盖率
仍可正常统计；同时用空闲端口 + 就绪轮询（含重试）规避端口竞态。

若设置环境变量 ``E2E_BASE_URL``，则不再在进程内启动 uvicorn，而是直连该外部
地址（例如经 ``docker compose`` 拉起的容器化后端），从而复用全部 E2E 用例
验证「真实部署产物」（见 CI 的 ``e2e-docker`` job）。
"""
from __future__ import annotations

import os
import socket
import sys
import threading
import time
from pathlib import Path

import httpx
import pytest
import uvicorn

# 在 importlib 导入模式下，pytest 不再把测试目录加入 sys.path，
# 因此同目录的裸模块（如 helpers）无法被 ``from helpers import ...`` 找到。
# 这里显式把本目录加入 sys.path，保持与 prepend 模式一致的可用行为。
_HERE = Path(__file__).resolve().parent
if str(_HERE) not in sys.path:
    sys.path.insert(0, str(_HERE))

from app.main import app


def _free_port() -> int:
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def _wait_ready(url: str, deadline: float) -> bool:
    while time.time() < deadline:
        try:
            if httpx.get(f"{url}/api/health", timeout=1).status_code == 200:
                return True
        except Exception:  # noqa: BLE001  服务尚未就绪
            time.sleep(0.1)
    return False


@pytest.fixture(scope="module")
def server_url():
    # 若指定了外部基址（如容器化部署），则直连该地址而不在本进程内启动 uvicorn，
    # 从而复用全部 E2E 用例验证「真实部署产物」（见 CI 的 e2e-docker job）。
    external = os.environ.get("E2E_BASE_URL")
    if external:
        if not _wait_ready(external, time.time() + 30):
            pytest.fail(f"E2E 外部服务在超时内未就绪：{external}")
        yield external.rstrip("/")
        return

    server = None
    thread = None
    url = None
    # 取空闲端口后由 uvicorn 绑定；带少量重试以规避极小概率的端口竞态
    for _ in range(5):
        port = _free_port()
        config = uvicorn.Config(
            app, host="127.0.0.1", port=port, log_level="error",
        )
        server = uvicorn.Server(config)
        thread = threading.Thread(target=server.run, daemon=True)
        thread.start()

        url = f"http://127.0.0.1:{port}"
        if _wait_ready(url, time.time() + 15):
            break
        server.should_exit = True
        thread.join(timeout=5)

    if url is None:
        pytest.fail("E2E 服务在超时内未能就绪")

    yield url

    if server is not None:
        server.should_exit = True
        thread.join(timeout=5)


@pytest.fixture(scope="module")
def client(server_url: str) -> httpx.Client:
    with httpx.Client(base_url=server_url, timeout=15) as c:
        yield c
