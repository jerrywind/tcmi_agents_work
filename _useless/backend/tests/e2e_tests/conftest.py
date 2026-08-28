"""e2e_tests 的 conftest：复用兄弟目录 tests/e2e 的服务启动与 httpx client fixtures。

tests/e2e/conftest.py 定义了在独立线程启动真实 uvicorn 的 server_url / client fixtures；
本目录与其约定一致，仅通过路径注入复用，避免重复维护启动逻辑。
"""
from __future__ import annotations

import sys
from pathlib import Path

_SIBLING = Path(__file__).resolve().parent.parent / "e2e"
if str(_SIBLING) not in sys.path:
    sys.path.insert(0, str(_SIBLING))

# 直接导入兄弟 conftest 中定义的 fixtures，使其在 e2e_tests 下可见
from conftest import client, server_url  # noqa: F401,E402
