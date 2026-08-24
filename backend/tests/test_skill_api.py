"""SKILL 运行时 API 测试：列表、热装载、热卸载、错误分支。"""
from fastapi.testclient import TestClient

from app.main import app

client = TestClient(app)


def test_list_skills_includes_builtin():
    r = client.get("/api/skills")
    assert r.status_code == 200
    body = r.json()
    names = {s["name"] for s in body["skills"]}
    assert "tcm-kb" in names
    tool_names = {t["name"] for t in body["tools"]}
    assert "lookup_syndrome_treatment" in tool_names


def test_unload_and_load_roundtrip():
    # 热卸载内置技能
    r = client.post("/api/skills/unload", json={"name": "tcm-kb"})
    assert r.status_code == 200 and r.json()["ok"] is True

    listed = client.get("/api/skills").json()
    assert "tcm-kb" not in {s["name"] for s in listed["skills"]}

    # 热装载回来
    r2 = client.post("/api/skills/load", json={"name": "tcm-kb"})
    assert r2.status_code == 200
    assert r2.json()["name"] == "tcm-kb"

    # 还原全局注册表状态
    listed2 = client.get("/api/skills").json()
    assert "tcm-kb" in {s["name"] for s in listed2["skills"]}


def test_load_requires_name_or_path():
    r = client.post("/api/skills/load", json={})
    assert r.status_code == 400


def test_load_unknown_name():
    r = client.post("/api/skills/load", json={"name": "does-not-exist"})
    assert r.status_code == 400


def test_unload_unknown():
    r = client.post("/api/skills/unload", json={"name": "nope"})
    assert r.status_code == 404
