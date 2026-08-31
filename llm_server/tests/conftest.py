"""llm_server 测试的公共前置：把 `llm_server/` 加入模块搜索路径。

这样 `pytest tests`（不管从哪个目录执行）都能 `import app.*`。
"""
from __future__ import annotations

import sys
from pathlib import Path

_ROOT = Path(__file__).resolve().parent.parent
if str(_ROOT) not in sys.path:
    sys.path.insert(0, str(_ROOT))
