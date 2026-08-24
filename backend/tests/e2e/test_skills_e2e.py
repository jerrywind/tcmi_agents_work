"""E2E：SKILL 运行时生命周期（启动自动发现 + 运行时热装载/卸载）。"""
from __future__ import annotations

import httpx
import pytest

from helpers import create_consultation

pytestmark = pytest.mark.e2e

# 一个最小可装载技能：含 1 个工具，归 treatment.plan 能力
SKILL_SRC = '''
from app.skills.types import SkillManifest, ToolSpec

SKILL = SkillManifest(
    name="e2e-temp-skill",
    version="0.1.0",
    description="E2E 临时技能",
    tools=[ToolSpec(
        name="e2e_ping",
        description="返回 pong",
        parameters={"type": "object", "properties": {}},
        capability="treatment.plan",
    )],
)

def e2e_ping() -> dict:
    return {"pong": True}

HANDLERS = {"e2e_ping": e2e_ping}
'''


def _write_skill(tmp_path) -> str:
    p = tmp_path / "e2e_temp_skill.py"
    p.write_text(SKILL_SRC, encoding="utf-8")
    return str(p)


def test_skill_lifecycle(client: httpx.Client, tmp_path):
    base = client.get("/api/skills").json()
    base_names = {s["name"] for s in base["skills"]}
    base_tool_count = len(base["tools"])
    assert "tcm-kb" in base_names  # 内置技能已在启动时自动发现并装载

    # 运行时按路径热装载
    path = _write_skill(tmp_path)
    r = client.post("/api/skills/load", json={"path": path})
    assert r.status_code == 200
    assert r.json()["name"] == "e2e-temp-skill"

    after = client.get("/api/skills").json()
    assert "e2e-temp-skill" in {s["name"] for s in after["skills"]}
    assert len(after["tools"]) == base_tool_count + 1
    assert "e2e_ping" in {t["name"] for t in after["tools"]}

    # 卸载
    u = client.post("/api/skills/unload", json={"name": "e2e-temp-skill"})
    assert u.status_code == 200 and u.json()["ok"] is True

    reset = client.get("/api/skills").json()
    assert "e2e-temp-skill" not in {s["name"] for s in reset["skills"]}
    assert len(reset["tools"]) == base_tool_count


def test_load_by_name_builtin(client: httpx.Client):
    r = client.post("/api/skills/load", json={"name": "tcm-kb"})
    assert r.status_code == 200
    assert r.json()["name"] == "tcm-kb"


def test_load_missing_returns_400(client: httpx.Client):
    r = client.post("/api/skills/load", json={"name": "does-not-exist"})
    assert r.status_code == 400


def test_unload_missing_returns_404(client: httpx.Client):
    r = client.post("/api/skills/unload", json={"name": "nope"})
    assert r.status_code == 404


def test_loaded_skill_does_not_break_consultation(client: httpx.Client, tmp_path):
    # 运行时装载技能后，新问诊仍应正常推进（技能注册不入主流程关键路径）
    path = _write_skill(tmp_path)
    client.post("/api/skills/load", json={"path": path})
    cid = create_consultation(client)
    body = client.post(f"/api/consultations/{cid}/start").json()
    assert body["status"] in (
        "running", "waiting_answer", "referred", "finished", "planning",
    )
    client.post("/api/skills/unload", json={"name": "e2e-temp-skill"})
