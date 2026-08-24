"""E2E 驱动辅助：用真实 HTTP 客户端推进一次完整问诊。"""
from __future__ import annotations

import httpx

COMPLAINT = "我最近口苦、口臭、大便粘马桶、身体困重、没胃口"


def create_consultation(client: httpx.Client, complaint: str = COMPLAINT,
                        gender: str = "男") -> str:
    r = client.post("/api/consultations",
                    json={"patient": {"gender": gender}, "complaint": complaint})
    assert r.status_code == 200, r.text
    return r.json()["id"]


def drive_to_finished(client: httpx.Client, cid: str, max_steps: int = 20) -> dict:
    """推进问诊直到 finished / referred，返回最终会话状态。"""
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
