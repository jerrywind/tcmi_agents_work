"""E2E：完整问诊生命周期、红旗转诊、图片上传与静态服务。"""
from __future__ import annotations

import httpx
import pytest

from helpers import create_consultation, drive_to_finished

pytestmark = pytest.mark.e2e


def test_full_consultation_lifecycle(client: httpx.Client):
    cid = create_consultation(client)
    body = drive_to_finished(client, cid)

    assert body["status"] == "finished"
    report = body["report"]
    assert isinstance(report["syndromes"], list) and len(report["syndromes"]) >= 1
    assert isinstance(report["treatments"], list) and len(report["treatments"]) >= 1

    # 报告可独立拉取
    r = client.get(f"/api/consultations/{cid}/report")
    assert r.status_code == 200
    assert len(r.json()["treatments"]) >= 1

    # trace 记录 sub-agent 调用轨迹（端到端可观测性）
    t = client.get(f"/api/consultations/{cid}/trace")
    assert t.status_code == 200
    trace = t.json()
    assert isinstance(trace, list) and len(trace) > 0
    assert any(x.get("capability") for x in trace)


def test_red_flag_referral(client: httpx.Client):
    cid = create_consultation(client, complaint="突然胸痛放射到左臂，大汗淋漓")
    r = client.post(f"/api/consultations/{cid}/start?sync=true")
    assert r.status_code == 200
    body = r.json()
    assert body["status"] == "referred"
    assert body["report"]["red_flag"]


def test_upload_image_and_serve(client: httpx.Client, tmp_path):
    cid = create_consultation(client)
    img = tmp_path / "tongue.jpg"
    img.write_bytes(b"\xff\xd8\xff\xe0mockjpeg")

    with open(img, "rb") as f:
        r = client.post(
            f"/api/consultations/{cid}/images",
            data={"type": "tongue"},
            files={"file": ("tongue.jpg", f, "image/jpeg")},
        )
    assert r.status_code == 200
    url = r.json()["url"]
    assert url.startswith("/uploads/")

    # 静态文件经真实 HTTP 返回
    g = client.get(url)
    assert g.status_code == 200
    assert g.content.startswith(b"\xff\xd8")


def test_upload_image_invalid_type(client: httpx.Client):
    cid = create_consultation(client)
    r = client.post(
        f"/api/consultations/{cid}/images",
        data={"type": "bad"},
        files={"file": ("x.jpg", b"x", "image/jpeg")},
    )
    assert r.status_code == 400
