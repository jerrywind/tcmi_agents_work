"""E2E：系统路由可观测性 + SKILL 工具暴露。"""
from __future__ import annotations

import httpx
import pytest

pytestmark = pytest.mark.e2e


def test_system_agents_routing(client: httpx.Client):
    r = client.get("/api/system/agents")
    assert r.status_code == 200
    agents = r.json()
    caps = {a["capability"] for a in agents}
    assert "diagnosis.differentiation" in caps
    assert "treatment.plan" in caps
    # 每个 capability 都暴露当前 impl 与可切换实现列表
    for a in agents:
        assert a["current_impl"]
        assert isinstance(a["available_impls"], list)


def test_skills_tools_exposed(client: httpx.Client):
    r = client.get("/api/skills")
    assert r.status_code == 200
    body = r.json()
    skill_names = {s["name"] for s in body["skills"]}
    assert "tcm-kb" in skill_names

    tool_caps = {t["name"]: t["capability"] for t in body["tools"]}
    # 内置技能的工具已归到 treatment.plan 能力，供诊疗 LLM 调用
    assert tool_caps.get("lookup_syndrome_treatment") == "treatment.plan"
    assert tool_caps.get("lookup_herb") == "treatment.plan"
