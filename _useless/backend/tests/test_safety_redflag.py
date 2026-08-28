"""T1.5：tcm-safety 红旗分级与就诊科室映射判定单测。

覆盖：
-  urgent 类红旗（如胸痛/咯血）→ level=urgent + 具体就诊科室
-  warning 类红旗（如持续高热/体重骤降）→ level=warning + 科室
-  模糊包含匹配（用户输入「最近总是胸痛放射左臂」能命中「胸痛」）
-  未命中信号 → 通用兜底（warning + 就近医疗机构），不漏报
-  安全 agent 多红旗扫描 + 分级标注一致性

``lookup_redflag`` 由 tcm-safety 技能提供，通过技能注册表运行，
确保走与运行时一致的装载路径。
"""
from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

from app.agents.safety import SafetyRuleAgent
from app.config import SKILLS_DIR
from app.protocol.base import Capability
from app.skills.loader import discover_skills
from app.skills.registry import skill_registry

# 确保技能已在注册表中（测试可能未导入 app.main，故显式发现一次）
if skill_registry.get_skill("tcm-safety") is None:
    discover_skills(SKILLS_DIR)

# tcm-safety 目录名含连字符，无法用常规 import；按文件路径加载模块取函数
_SKILL_INIT = Path(__file__).resolve().parents[1] / "app" / "skills" / "tcm-safety" / "__init__.py"
_spec = importlib.util.spec_from_file_location("tcm_safety_mod", str(_SKILL_INIT))
_tcm_safety = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_tcm_safety)  # type: ignore[union-attr]
lookup_redflag = _tcm_safety.lookup_redflag


def test_skill_registered():
    assert skill_registry.get_skill("tcm-safety") is not None


# ---------------- 分级 + 科室映射 ----------------
def test_urgent_cardiac_redflag():
    g = lookup_redflag("胸痛")
    assert g["matched"] is True
    info = g["guidance"][0]
    assert info["level"] == "urgent"
    assert "胸痛中心" in info["where"]
    assert "冠脉" in info["action"]


def test_urgent_hemoptysis_redflag():
    g = lookup_redflag("咯血")
    assert g["matched"] is True
    info = g["guidance"][0]
    assert info["level"] == "urgent"
    assert "呼吸" in info["where"]


def test_warning_fever_redflag():
    g = lookup_redflag("持续高热")
    assert g["matched"] is True
    info = g["guidance"][0]
    assert info["level"] == "warning"
    assert "发热门诊" in info["where"] or "感染" in info["where"]


def test_warning_weight_loss_redflag():
    g = lookup_redflag("体重骤降")
    assert g["matched"] is True
    info = g["guidance"][0]
    assert info["level"] == "warning"


# ---------------- 模糊包含匹配（用户输入含红旗关键词） ----------------
def test_partial_match_contains_keyword():
    # 用户口语化描述，应包含「胸痛」关键词
    g = lookup_redflag("我最近老觉得胸痛放射到左臂")
    assert g["matched"] is True
    assert any("胸痛" == m["signal"] for m in g["guidance"])


# ---------------- 未命中兜底：不漏报 ----------------
def test_unmatched_falls_back_to_general():
    g = lookup_redflag("最近只是有点累")
    assert g["matched"] is False
    info = g["guidance"][0]
    assert info["level"] == "warning"
    assert "就近" in info["where"]


def test_empty_signal_falls_back():
    g = lookup_redflag("")
    assert g["matched"] is False
    assert g["guidance"]


# ---------------- 经注册表运行：与运行时装载一致 ----------------
async def test_lookup_via_registry_returns_guidance():
    out = await skill_registry.run_tool("lookup_redflag", {"signal": "咯血"})
    assert out["matched"] is True
    assert out["guidance"][0]["level"] == "urgent"


# ---------------- 安全 Agent 端到端扫描 ----------------
async def test_safety_agent_multiple_red_flags(build_req):
    text = "我突然胸痛而且还在咯血，有点呼吸困难"
    resp = await SafetyRuleAgent().handle(
        build_req(Capability.SAFETY, payload={"text": text}))
    assert resp.status == "ok"
    # 至少命中胸痛 / 咯血 / 呼吸困难 三类红旗
    reasons = " ".join(a.reason for a in resp.alerts)
    assert "胸痛" in reasons
    assert "咯血" in reasons
    assert "呼吸困难" in reasons
    # 规则实现统一标注为 danger 级，且都给出就医建议
    assert all(a.level == "danger" for a in resp.alerts)
    assert all("就诊" in a.advice for a in resp.alerts)


async def test_safety_agent_urgent_flag_present(build_req):
    resp = await SafetyRuleAgent().handle(
        build_req(Capability.SAFETY, payload={"text": "出现意识模糊和抽搐"}))
    assert resp.status == "ok"
    reasons = " ".join(a.reason for a in resp.alerts)
    assert "意识模糊" in reasons and "抽搐" in reasons
