"""全链路 e2e 共享辅助：驱动问诊与读取产物。

与 backend/tests/e2e/helpers.py 风格一致，但本文件自包含、不依赖 backend
已有的测试 fixture，方便在 tcm_work/e2e_tests 下独立运行。
所有 *sync* 读取函数接收 base_url 字符串，自行构造 httpx.Client；
drive_to_finished 接收已构造的 client（复用其超时/base_url 配置）。
"""
from __future__ import annotations

import time

import httpx


def create_consultation(client: httpx.Client, complaint: str, gender: str = "男") -> str:
    r = client.post("/api/consultations",
                    json={"patient": {"gender": gender}, "complaint": complaint})
    assert r.status_code == 200, r.text
    return r.json()["id"]


def drive_to_finished(client: httpx.Client, cid: str, max_steps: int = 20) -> dict:
    r = client.post(f"/api/consultations/{cid}/start?sync=true")
    assert r.status_code == 200, r.text
    body = r.json()
    steps = 0
    while body["status"] in ("waiting_answer", "treatment_qa") and steps < max_steps:
        q = body["question"]
        assert q is not None, f"等待作答但无问题（step={steps}）"
        opts = q.get("options") or [{"value": "无"}]
        r = client.post(f"/api/consultations/{cid}/answer?sync=true",
                        json={"question_id": q["id"], "value": opts[0]["value"]})
        assert r.status_code == 200, r.text
        body = r.json()
        steps += 1
    return body


def get_status(base_url: str, cid: str) -> str:
    with httpx.Client(base_url=base_url, timeout=10) as c:
        return c.get(f"/api/consultations/{cid}").json()["status"]


def get_report(base_url: str, cid: str) -> dict:
    with httpx.Client(base_url=base_url, timeout=10) as c:
        return c.get(f"/api/consultations/{cid}/report").json()


def get_evidences(base_url: str, cid: str) -> list:
    with httpx.Client(base_url=base_url, timeout=10) as c:
        return c.get(f"/api/consultations/{cid}/evidences").json()


def get_trace(base_url: str, cid: str) -> list:
    with httpx.Client(base_url=base_url, timeout=10) as c:
        return c.get(f"/api/consultations/{cid}/trace").json()


def upload_image(base_url: str, cid: str, img_path: str) -> str:
    with httpx.Client(base_url=base_url, timeout=30) as c:
        with open(img_path, "rb") as f:
            r = c.post(f"/api/consultations/{cid}/image",
                       files={"file": (img_path.rsplit("/", 1)[-1], f, "image/jpeg")})
        assert r.status_code == 200, r.text
        return r.json()["id"]


def wait_until_finished(base_url: str, cid: str, timeout: float = 60) -> None:
    """轮询会话状态直到 finished/referred，超时即失败。"""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            st = get_status(base_url, cid)
        except Exception:
            st = None
        if st in ("finished", "referred"):
            return
        time.sleep(1.0)
    raise TimeoutError(f"会话 {cid} 在 {timeout}s 内未到达 finished/referred")
