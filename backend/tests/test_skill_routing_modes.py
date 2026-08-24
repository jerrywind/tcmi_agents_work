"""T1.3 联调：SKILL 管理接口在 rule 与 llm 两种路由模式下行为一致。

skills 的装载/卸载/列表与 Sub-Agent 路由（TCM_ROUTING_FILE）解耦，
本测试确保切换 routing.yaml / routing.llm.yaml 后 /api/skills 全链路无回归。
"""
import os
from pathlib import Path

import pytest
import yaml

BACKEND = Path(__file__).resolve().parent.parent


@pytest.fixture
def client_for_routing(monkeypatch):
    """在指定路由文件下构建 app 的 TestClient。

    关键：直接切换 ``settings.routing`` / ``settings.llm`` 字典，**不删除任何
    ``app.*`` 模块缓存**。删除 ``app.*`` 会触发模块重加载，使 agent 类（如
    ``ListeningLLMAgent``）在「测试收集时导入的引用」与「运行时 sys.modules 中的
    模块」之间出现身份分裂，导致后续用例对 ``get_provider`` 的 monkeypatch 命中
    错误副本（单跑通过、全量失败的根因）。这里复用已导入的 ``app.main.app``，
    路由读 ``settings.route_of`` 是动态方法，切换 settings 即生效。
    """
    from app.config import settings
    from app.main import app
    from fastapi.testclient import TestClient

    def _make(routing_file: str):
        data = yaml.safe_load(
            (BACKEND / "app" / routing_file).read_text(encoding="utf-8"))
        # 仅覆盖与路由相关的字段，保持 settings 对象身份稳定
        if "routing" in data:
            monkeypatch.setattr(settings, "routing", data["routing"])
        if "llm" in data:
            monkeypatch.setattr(settings, "llm", data["llm"])
        return TestClient(app)

    yield _make


def test_skills_list_rule_mode(client_for_routing):
    client = client_for_routing("routing.yaml")
    r = client.get("/api/skills")
    assert r.status_code == 200
    body = r.json()
    assert body["skills_dir"]
    assert any(s["name"] == "tcm-kb" for s in body["skills"])
    assert any(t["name"] == "lookup_syndrome_treatment" for t in body["tools"])


def test_skills_list_llm_mode(client_for_routing):
    client = client_for_routing("routing.llm.yaml")
    r = client.get("/api/skills")
    assert r.status_code == 200
    body = r.json()
    names = {s["name"] for s in body["skills"]}
    # llm 模式下内置技能同样应被发现装载
    assert "tcm-kb" in names
    assert any(t["name"] == "lookup_syndrome_treatment" for t in body["tools"])


def test_skills_roundtrip_both_modes(client_for_routing):
    for rf in ("routing.yaml", "routing.llm.yaml"):
        client = client_for_routing(rf)
        # 卸载
        r = client.post("/api/skills/unload", json={"name": "tcm-kb"})
        assert r.status_code == 200 and r.json()["ok"] is True
        # 装载回来
        r2 = client.post("/api/skills/load", json={"name": "tcm-kb"})
        assert r2.status_code == 200 and r2.json()["name"] == "tcm-kb"
        # 错误分支
        assert client.post("/api/skills/load", json={}).status_code == 400
        assert client.post("/api/skills/unload", json={"name": "nope"}).status_code == 404


def test_skills_load_by_path_rule_and_llm(client_for_routing):
    for rf in ("routing.yaml", "routing.llm.yaml"):
        client = client_for_routing(rf)
        skill_path = BACKEND / "app" / "skills" / "tcm-rag"
        r = client.post("/api/skills/load", json={"path": str(skill_path)})
        # 装载成功或已装载（幂等），均不应 5xx
        assert r.status_code in (200, 400)
