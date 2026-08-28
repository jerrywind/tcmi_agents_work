"""E2E：图片驱动的完整问诊（真实舌象图 + 真实多模态模型）。

素材：``images/tongue_A_Tongue.jpg``
  - 来源：Wikimedia Commons 「File:A Tongue.jpg」
  - 作者：Stephanie cheks（Own work）
  - 许可：Creative Commons Attribution-Share Alike 4.0 International（CC BY-SA 4.0）
    https://creativecommons.org/licenses/by-sa/4.0/
  - 下载：https://upload.wikimedia.org/wikipedia/commons/b/ba/A_Tongue.jpg

设计：
- 流程冒烟（始终执行）：上传真实舌象图 → 发起完整问诊（sync）→ 断言到达 finished 且
  产出报告。即使视觉模型不可用，诊断逻辑亦会走 rule 兜底，保证「流程能跑通」。
- 真·图片消费（模型可达时执行）：探测 TCM_LLM_BASE_URL 是否可达（或设 RUN_E2E_VISION=1），
  可达则从 trace 中确认 diagnosis.inspection 调用了 tcm-vision（望诊分析非空），
  且最终报告含望诊相关特征，验证图片确实被多模态模型消费。
"""
from __future__ import annotations

import os
import sys
from pathlib import Path

import httpx
import pytest
from pathlib import Path as _Path

# helpers 位于兄弟目录 tests/e2e/（conftest 仅把该目录加入 sys.path），此处显式补齐
_HERE = _Path(__file__).resolve().parent
_E2E_DIR = _HERE.parent / "e2e"
if str(_E2E_DIR) not in sys.path:
    sys.path.insert(0, str(_E2E_DIR))
from helpers import create_consultation, drive_to_finished  # noqa: E402

pytestmark = pytest.mark.e2e

_IMAGE = _HERE / "images" / "tongue_A_Tongue.jpg"
_LICENSE_NOTE = (
    "图片素材 tongue_A_Tongue.jpg：CC BY-SA 4.0，作者 Stephanie cheks，"
    "来自 Wikimedia Commons (File:A…)。"
)

# 触发「口苦/口臭/便粘」类主诉，让望诊（舌象）成为辨证关键证据之一
_COMPLAINT = "最近口苦口臭、大便粘马桶、身体困重、没胃口，请结合舌象看看"


def _vision_available() -> bool:
    """探测多模态上游是否可达；或显式用 RUN_E2E_VISION=1 强制开启。"""
    if os.environ.get("RUN_E2E_VISION") == "1":
        return True
    base = os.environ.get("TCM_LLM_BASE_URL", "").rstrip("/")
    if not base:
        return False
    key = os.environ.get("TCM_LLM_API_KEY", "sk-noauth")
    try:
        with httpx.Client(timeout=8) as c:
            r = c.get(f"{base}/models",
                      headers={"Authorization": f"Bearer {key}"})
        return r.status_code == 200
    except Exception:  # noqa: BLE001  网络不通则视为不可用
        return False


from dataclasses import dataclass


@dataclass
class _Uploaded:
    image_id: str
    cid: str


@pytest.fixture(scope="module")
def uploaded(client: httpx.Client) -> _Uploaded:
    """上传真实舌象图，返回图片 id 与所属会话 id；不可上传则跳过。"""
    if not _IMAGE.exists():
        pytest.skip(f"测试素材缺失：{_IMAGE}")
    cid = create_consultation(client, complaint=_COMPLAINT)
    with open(_IMAGE, "rb") as f:
        r = client.post(
            f"/api/consultations/{cid}/images",
            data={"type": "tongue"},
            files={"file": ("tongue_A_Tongue.jpg", f, "image/jpeg")},
        )
    assert r.status_code == 200, r.text
    return _Uploaded(image_id=r.json()["id"], cid=cid)


def test_upload_real_tongue_image(uploaded):
    # 验证素材已就绪 + 图片成功入库
    assert uploaded.image_id.startswith("img_")
    assert _IMAGE.exists() and _IMAGE.stat().st_size > 4096, "素材应是一张真实图片"


def test_consultation_with_image_runs_to_finished(client: httpx.Client, uploaded):
    """核心流程：带真实舌象图的完整问诊能跑到 finished 并产出报告。"""
    cid = uploaded.cid
    body = drive_to_finished(client, cid)

    assert body["status"] == "finished", body.get("report")
    report = body["report"]
    assert isinstance(report["syndromes"], list) and len(report["syndromes"]) >= 1
    assert isinstance(report["treatments"], list) and len(report["treatments"]) >= 1


@pytest.mark.skipif(not _vision_available(),
                    reason="多模态模型不可达（设 RUN_E2E_VISION=1 或保证 TCM_LLM_BASE_URL 可达可开启）")
def test_image_is_consumed_by_vision_skill(client: httpx.Client, uploaded):
    """图片确实进入望诊（vision）管线并被多模态模型分析。

    确定性断言（每次必过，只要上游可达）：
      - 会诊 trace 中存在 diagnosis.inspection 调用（说明舌象图被送入望诊阶段）；
      - 报告落地（test_consultation_with_image_runs_to_finished 已证 finished）。
    软性校验（信息性，不阻断）：若产生「望」来源证据，则其应含舌象相关特征，
      证明图片被模型真正读取而非仅占位。
    """
    cid = uploaded.cid

    t = client.get(f"/api/consultations/{cid}/trace")
    assert t.status_code == 200
    caps = {x.get("capability") for x in t.json()}
    assert any("inspection" in (c or "") for c in caps), \
        f"trace 中无 inspection（望诊）调用，说明图片未进入视觉管线：{sorted(caps)}"

    # 软性校验：模型是否真正读取了舌象（非确定性，仅信息性）
    body = client.get(f"/api/consultations/{cid}").json()
    tongue_ev = [e for e in body.get("evidences", []) if e.get("source") == "望"]
    if tongue_ev:
        desc = " ".join(e.get("value", "") or e.get("desc", "") for e in tongue_ev)
        grounded = any(k in (desc + body.get("report", {}).get("reasoning", ""))
                       for k in ("舌", "苔", "淡红", "红", "白", "腻", "面色", "形"))
        assert grounded, f"望诊证据未见舌象特征落地：{desc[:200]}"
