"""FastAPI 接口测试：用 TestClient 覆盖健康、会话、启动、问答、上传与报告。"""
from fastapi.testclient import TestClient

from app.main import app

COMPLAINT = "我最近口苦、口臭、大便粘马桶、身体困重、没胃口"


def _client():
    return TestClient(app)


def _create(complaint=COMPLAINT, gender="男"):
    c = _client()
    r = c.post("/api/consultations",
               json={"patient": {"gender": gender}, "complaint": complaint})
    return c, r.json()["id"]


def test_health():
    r = _client().get("/api/health")
    assert r.status_code == 200
    assert r.json()["ok"] is True


def test_create_and_get():
    c, cid = _create()
    r = c.get(f"/api/consultations/{cid}")
    assert r.status_code == 200
    assert r.json()["status"] == "created"


def test_start_red_flag():
    c, cid = _create(complaint="胸痛放射到左臂大汗淋漓")
    r = c.post(f"/api/consultations/{cid}/start?sync=true")
    assert r.status_code == 200
    assert r.json()["status"] == "referred"


def test_start_then_full_conversation():
    c, cid = _create()
    r = c.post(f"/api/consultations/{cid}/start?sync=true")
    assert r.status_code == 200
    body = r.json()

    # 驱动多轮（问询 + 方案追问）直到结束
    steps = 0
    while body["status"] in ("waiting_answer", "treatment_qa") and steps < 20:
        q = body["question"]
        r2 = c.post(f"/api/consultations/{cid}/answer?sync=true",
                    json={"question_id": q["id"], "value": q["options"][0]["value"]})
        body = r2.json()
        steps += 1

    assert body["status"] == "finished"
    assert body["report"]["treatments"]


def test_answer_not_waiting():
    c, cid = _create()  # 状态 created
    r = c.post(f"/api/consultations/{cid}/answer",
               json={"question_id": "q", "value": "v"})
    assert r.status_code == 400


def test_report_not_ready():
    c, cid = _create()
    r = c.get(f"/api/consultations/{cid}/report")
    assert r.status_code == 404


def test_upload_image(tmp_path):
    c, cid = _create()
    files = {"file": ("t.jpg", b"binarydata", "image/jpeg")}
    r = c.post(f"/api/consultations/{cid}/images", data={"type": "tongue"}, files=files)
    assert r.status_code == 200
    assert r.json()["url"].startswith("/uploads/")


def test_upload_image_invalid_type():
    c, cid = _create()
    r = c.post(f"/api/consultations/{cid}/images", data={"type": "bad"},
               files={"file": ("t.jpg", b"x", "image/jpeg")})
    assert r.status_code == 400


def test_system_agents_and_trace():
    c, cid = _create(complaint="胸痛放射左臂大汗")
    c.post(f"/api/consultations/{cid}/start")
    r = c.get("/api/system/agents")
    assert r.status_code == 200
    caps = {x["capability"] for x in r.json()}
    assert "treatment.plan" in caps

    t = c.get(f"/api/consultations/{cid}/trace")
    assert t.status_code == 200
    assert isinstance(t.json(), list)


def test_family_member_flow():
    c = _client()
    # 创建家庭，自动含"本人"
    r = c.post("/api/families", json={"name": "我的家庭"})
    assert r.status_code == 200
    f = r.json()
    assert f["name"] == "我的家庭"
    assert any(m["relation"] == "本人" for m in f["members"])
    fid = f["id"]

    # 添加成员
    r = c.post(f"/api/families/{fid}/members",
               json={"name": "父亲", "relation": "父亲",
                     "patient": {"age": 60, "gender": "男"}})
    assert r.status_code == 200
    mid = r.json()["id"]

    # 为成员创建问诊并归属
    r = c.post("/api/consultations",
               json={"patient": {"gender": "男", "age": 60},
                     "complaint": "咳嗽", "family_id": fid, "member_id": mid})
    assert r.status_code == 200
    cid = r.json()["id"]

    # 按家庭列出
    r = c.get(f"/api/families/{fid}/consultations")
    assert r.status_code == 200
    items = r.json()
    assert len(items) == 1
    assert items[0]["member_id"] == mid
    assert items[0]["id"] == cid

    # 按成员过滤
    r = c.get(f"/api/families/{fid}/consultations?member_id={mid}")
    assert r.status_code == 200
    assert len(r.json()) == 1

    # 更新成员
    r = c.patch(f"/api/families/{fid}/members/{mid}",
                json={"name": "父亲", "relation": "父亲",
                      "patient": {"age": 61, "gender": "男"}, "note": "高血压"})
    assert r.status_code == 200
    assert r.json()["patient"]["age"] == 61
    assert r.json()["note"] == "高血压"


def test_family_not_found():
    c = _client()
    r = c.get("/api/families/nope")
    assert r.status_code == 404


def test_create_consultation_links_family():
    c = _client()
    r = c.post("/api/families", json={"name": "f"})
    fid = r.json()["id"]
    r = c.post(f"/api/families/{fid}/members",
               json={"name": "本人", "relation": "本人"})
    mid = r.json()["id"]
    r = c.post("/api/consultations",
               json={"patient": {}, "complaint": "头痛",
                     "family_id": fid, "member_id": mid})
    cid = r.json()["id"]
    r = c.get(f"/api/consultations/{cid}")
    assert r.status_code == 200
    assert r.json()["family_id"] == fid
    assert r.json()["member_id"] == mid


def test_ppg_simulate_and_evidence():
    c = _client()
    r = c.post("/api/consultations",
               json={"patient": {"gender": "男"}, "complaint": "疲劳"})
    cid = r.json()["id"]
    # 模拟滑脉
    r = c.post(f"/api/consultations/{cid}/ppg",
               json={"simulate": True, "profile": "slippery", "rate_bpm": 82})
    assert r.status_code == 200
    body = r.json()
    assert body["ppg"] is not None
    assert body["ppg"]["shape"] == "滑"
    assert body["ppg"]["rate_bpm"] > 0
    # 证据池出现脉象证据
    pulse_evs = [e for e in body["evidences"] if e["source"] == "切" and e["key"].startswith("pulse.")]
    assert len(pulse_evs) >= 4
    # 重新拉取后仍在
    r = c.get(f"/api/consultations/{cid}")
    assert r.json()["ppg"]["shape"] == "滑"


def test_ppg_real_samples():
    c = _client()
    from app.knowledge.ppg import synthesize_ppg
    r = c.post("/api/consultations",
               json={"patient": {"gender": "女"}, "complaint": "心悸"})
    cid = r.json()["id"]
    samples = synthesize_ppg(fs=50, duration_s=10, rate_bpm=68, profile="weak", seed=7)
    r = c.post(f"/api/consultations/{cid}/ppg",
               json={"samples": samples, "fs": 50})
    assert r.status_code == 200
    assert abs(r.json()["ppg"]["rate_bpm"] - 68) < 3
    assert r.json()["ppg"]["force"] in ("无力", "和缓")
