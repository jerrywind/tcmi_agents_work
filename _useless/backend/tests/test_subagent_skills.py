"""Sub-Agent 的 LLM 实现与技能（skill）测试。

覆盖：
- 各能力均有对应 system prompt（prompts.PROMPTS）。
- 闻/切/安全/问诊 LLM agent 的成功路径与离线兜底（现已全部接入 run_tool_loop）。
- 全部技能处理函数：tcm-reference / tcm-vision（既有）+ tcm-diet / tcm-auscultation /
  tcm-palpation / tcm-safety / tcm-inquiry / tcm-rag（扩展）。
- registry.tools_for 支持 capability 为列表的匹配。
- routing.llm.yaml 配置合法且默认指向 qwen3.6-9B + 各 LLM 实现。
"""
import importlib.util
import json
import pathlib

import pytest
import yaml

from app.agents.differentiation import DifferentiationLLMAgent
from app.agents.inspection import InspectionVisionAgent
from app.agents.listening import ListeningLLMAgent
from app.agents.palpation import PalpationLLMAgent
from app.agents.safety import SafetyLLMAgent
from app.agents.treatment import TreatmentLLMAgent
from app.agents.prompts import PROMPTS, system_prompt
from app.agents.skills_map import AGENT_SKILLS, skills_for
from app.knowledge.syndromes import SYNDROMES
from app.protocol.base import Capability
from app.protocol.llm import MockProvider, get_provider

_SKILLS_DIR = pathlib.Path(__file__).resolve().parent.parent / "app" / "skills"


def _load_skill_module(import_name: str, dir_name: str):
    """技能目录名含连字符，无法用常规包导入，按文件路径加载。"""
    path = _SKILLS_DIR / dir_name / "__init__.py"
    spec = importlib.util.spec_from_file_location(import_name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


_tcm_ref = _load_skill_module("tcm_reference_test", "tcm-reference")
_tcm_vis = _load_skill_module("tcm_vision_test", "tcm-vision")
lookup_syndrome_patterns = _tcm_ref.lookup_syndrome_patterns
analyze_tongue_image = _tcm_vis.analyze_tongue_image

# 本次扩展的新技能模块与处理函数
_tcm_diet = _load_skill_module("tcm_diet_test", "tcm-diet")
_tcm_aus = _load_skill_module("tcm_auscultation_test", "tcm-auscultation")
_tcm_pal = _load_skill_module("tcm_palpation_test", "tcm-palpation")
_tcm_saf = _load_skill_module("tcm_safety_test", "tcm-safety")
_tcm_inq = _load_skill_module("tcm_inquiry_test", "tcm-inquiry")
_tcm_rag = _load_skill_module("tcm_rag_test", "tcm-rag")
lookup_diet_therapy = _tcm_diet.lookup_diet_therapy
lookup_voice_pattern = _tcm_aus.lookup_voice_pattern
lookup_odor_pattern = _tcm_aus.lookup_odor_pattern
lookup_pulse_pattern = _tcm_pal.lookup_pulse_pattern
lookup_abdomen_pattern = _tcm_pal.lookup_abdomen_pattern
lookup_redflag = _tcm_saf.lookup_redflag
lookup_inquiry_focus = _tcm_inq.lookup_inquiry_focus
suggest_followup = _tcm_inq.suggest_followup
rag_text_retrieve = _tcm_rag.rag_text_retrieve


class FakeProvider:
    def __init__(self, text):
        self.text = text

    async def chat(self, messages, model=None, json_mode=False, tools=None):
        # T1.4 起 chat 返回 (content, usage) 二元组
        return self.text, {}

    @property
    def name(self):
        return "FakeProvider"


# ---------------- system prompt 覆盖 ----------------
def test_prompts_cover_all_capabilities():
    for cap in Capability:
        assert cap in PROMPTS, f"缺少 {cap} 的 system prompt"
        assert system_prompt(cap).strip()


# ---------------- 闻诊 LLM ----------------
async def test_listening_llm_success(monkeypatch, build_req):
    fp = FakeProvider(json.dumps(
        {"evidences": [{"category": "voice", "value": "声低息微", "confidence": 0.6}],
         "notes": ""}))
    # 直接 patch ListeningLLMAgent 所属模块的 get_provider，避免 importlib 模式下
    # 模块被重加载后「类归属模块」与「运行时 sys.modules 模块」不一致导致 monkeypatch
    # 字符串路径命中错误副本（单跑通过、全量失败的根因）。
    import sys
    mod = sys.modules[ListeningLLMAgent.__module__]
    monkeypatch.setattr(mod, "get_provider", lambda: fp)
    monkeypatch.setattr("app.protocol.llm._provider", None)
    # 隔离全局 skill 注册表：本用例验证「无工具 -> 单次 json_mode」的 LLM 路径，
    # 不依赖内置闻诊/听诊 skills 是否装载（否则 FakeProvider 在 function-calling
    # 分支下会返回空，导致 evidences 为空）。
    from app.skills.registry import skill_registry as _SR
    monkeypatch.setattr(_SR, "tools_for", lambda cap: [])
    req = build_req(Capability.LISTENING, payload={"text": "患者说话声音低微"})
    resp = await ListeningLLMAgent().handle(req)
    assert any(e.key == "voice" for e in resp.evidences)


async def test_listening_llm_offline_fallback(monkeypatch, build_req):
    # 无模型时优雅降级为规则结果，不抛错
    monkeypatch.setattr("app.agents.listening.get_provider", lambda: MockProvider())
    req = build_req(Capability.LISTENING, payload={"text": "最近口臭明显"})
    resp = await ListeningLLMAgent().handle(req)
    assert resp.capability == Capability.LISTENING


# ---------------- 切诊 LLM ----------------
async def test_palpation_llm_success(monkeypatch, build_req):
    fp = FakeProvider(json.dumps(
        {"evidences": [{"category": "pulse.quality", "value": "脉细", "confidence": 0.5}],
         "notes": ""}))
    import sys
    mod = sys.modules[PalpationLLMAgent.__module__]
    monkeypatch.setattr(mod, "get_provider", lambda: fp)
    monkeypatch.setattr("app.protocol.llm._provider", None)
    from app.skills.registry import skill_registry as _SR
    monkeypatch.setattr(_SR, "tools_for", lambda cap: [])
    req = build_req(Capability.PALPATION, payload={"text": "自觉脉搏细弱"})
    resp = await PalpationLLMAgent().handle(req)
    assert any(e.key == "pulse.quality" for e in resp.evidences)


# ---------------- 安全 LLM（规则 + 语义双重） ----------------
async def test_safety_llm_semantic_alert(monkeypatch, build_req):
    fp = FakeProvider(json.dumps(
        {"safe": False, "alerts": [{"level": "urgent", "signal": "咯血", "detail": "咳血"}]}))
    monkeypatch.setattr("app.agents.safety.get_provider", lambda: fp)
    req = build_req(Capability.SAFETY, payload={"text": "我最近咯血"})
    resp = await SafetyLLMAgent().handle(req)
    assert any("咯血" in a.reason for a in resp.alerts)
    # 规则网始终执行：即便 LLM 没报，关键词也应命中
    assert resp.alerts


async def test_safety_llm_offline_rule_only(monkeypatch, build_req):
    monkeypatch.setattr("app.agents.safety.get_provider", lambda: MockProvider())
    req = build_req(Capability.SAFETY, payload={"text": "最近剧烈胸痛"})
    resp = await SafetyLLMAgent().handle(req)
    assert any("胸痛" in a.reason for a in resp.alerts)


# ---------------- 技能：tcm-reference ----------------
async def test_tcm_reference_lookup_known():
    name = next(iter(SYNDROMES))
    res = await lookup_syndrome_patterns(name)
    assert res["found"] is True
    assert res["patterns"]


async def test_tcm_reference_lookup_empty():
    res = await lookup_syndrome_patterns("")
    assert res["found"] is False


# ---------------- 技能：tcm-vision（离线不崩溃） ----------------
async def test_tcm_vision_offline_no_crash(monkeypatch):
    import app.protocol.llm as llm_mod
    monkeypatch.setattr(llm_mod, "_provider", None)
    monkeypatch.setitem(llm_mod.settings.llm, "base_url", "")
    monkeypatch.setenv("TCM_LLM_API_KEY", "")
    res = await analyze_tongue_image("/x.jpg")
    assert isinstance(res, dict) and "ok" in res


# ---------------- routing.llm.yaml 配置校验 ----------------
def test_routing_llm_file_valid():
    from app.config import BASE_DIR
    data = yaml.safe_load((BASE_DIR / "routing.llm.yaml").read_text(encoding="utf-8"))
    r = data["routing"]
    assert r["diagnosis.inspection"]["impl"] == "llm_vision"
    assert r["diagnosis.differentiation"]["impl"] == "llm"
    assert r["treatment.plan"]["impl"] == "llm"
    assert r["diagnosis.safety"]["impl"] == "llm"
    assert data["llm"]["models"]["text-default"] == "qwen3.6-9B"
    # 望诊（图像理解）默认走 Qwen3-VL 原生多模态，可独立部署到专属视觉端点
    assert data["llm"]["models"]["vision-default"] == "Qwen3-VL-8B"


# ---------------- 全部 LLM agent 在离线（MockProvider）下不抛错 ----------------
async def test_all_llm_agents_offline_smoke(monkeypatch, build_req):
    monkeypatch.setattr("app.agents.differentiation.get_provider", lambda: MockProvider())
    monkeypatch.setattr("app.agents.treatment.get_provider", lambda: MockProvider())
    monkeypatch.setattr("app.agents.inspection.get_provider", lambda: MockProvider())
    for agent, cap, payload in [
        (DifferentiationLLMAgent(), Capability.DIFFERENTIATION, {"diagnoses": []}),
        (TreatmentLLMAgent(), Capability.TREATMENT, {"diagnoses": []}),
        (InspectionVisionAgent(), Capability.INSPECTION, {"images": []}),
    ]:
        resp = await agent.handle(build_req(cap, payload=payload))
        assert resp.capability == cap


# ---------------- 各 agent 的技能映射（skills_map） ----------------
def test_skills_map_covers_all_capabilities():
    for cap in Capability:
        assert cap in AGENT_SKILLS


def test_skills_map_assignments():
    # 每个 sub-agent 现在都至少拥有一个技能
    assert "tcm-kb" in AGENT_SKILLS[Capability.TREATMENT]
    assert "tcm-diet" in AGENT_SKILLS[Capability.TREATMENT]
    assert "tcm-rag" in AGENT_SKILLS[Capability.TREATMENT]
    assert "tcm-reference" in AGENT_SKILLS[Capability.DIFFERENTIATION]
    assert "tcm-rag" in AGENT_SKILLS[Capability.DIFFERENTIATION]
    assert "tcm-vision" in AGENT_SKILLS[Capability.INSPECTION]
    assert "tcm-rag" in AGENT_SKILLS[Capability.INSPECTION]
    assert "tcm-auscultation" in AGENT_SKILLS[Capability.LISTENING]
    assert "tcm-inquiry" in AGENT_SKILLS[Capability.INQUIRY]
    assert "tcm-rag" in AGENT_SKILLS[Capability.INQUIRY]
    assert "tcm-palpation" in AGENT_SKILLS[Capability.PALPATION]
    assert "tcm-safety" in AGENT_SKILLS[Capability.SAFETY]
    # 没有 agent 被分配空技能列表
    for cap in Capability:
        assert skills_for(cap), f"{cap} 未被分配任何技能"


# ---------------- 扩展技能的 handler 冒烟测试 ----------------
async def test_tcm_diet_handler():
    res = lookup_diet_therapy("脾胃湿热")
    assert res["syndrome"] == "脾胃湿热"
    assert res["diet_therapy"]
    # 未知证候回退到通用原则
    gen = lookup_diet_therapy("不存在的证候")
    assert gen["diet_therapy"]


async def test_tcm_auscultation_handler():
    res = lookup_voice_pattern("声音低")
    assert res["matches"]
    res2 = lookup_odor_pattern("口臭")
    assert res2["matches"]


async def test_tcm_palpation_handler():
    res = lookup_pulse_pattern("脉细")
    assert res["matches"]
    res2 = lookup_abdomen_pattern("拒按")
    assert res2["matches"]


async def test_tcm_safety_handler():
    res = lookup_redflag("胸痛")
    assert res["matched"] and res["guidance"][0]["level"] == "urgent"
    # 未命中信号也要回退（不漏报）
    miss = lookup_redflag("不明症状")
    assert miss["matched"] is False and miss["guidance"]


async def test_tcm_inquiry_handler():
    focus = lookup_inquiry_focus("脾胃湿热")
    assert focus["focus"] and focus["focus"][0]["feature_key"]
    follow = suggest_followup("口苦，身体困重，小便黄")
    assert follow["candidate_syndromes"]
    assert follow["suggested_next_question"] is not None


async def test_tcm_rag_handler_graceful(monkeypatch):
    # RAG 服务不可用时优雅降级（不抛异常、ok=false）
    async def _fake_post(path, payload):
        return {"ok": False, "results": [], "reason": "test"}
    monkeypatch.setattr(_tcm_rag, "_post", _fake_post)
    res = await rag_text_retrieve("肝郁脾虚怎么调理")
    assert res == {"ok": False, "results": [], "reason": "test"}


# ---------------- registry.tools_for 支持 capability 列表 ----------------
def test_registry_tools_for_list_capability():
    from app.skills.registry import skill_registry, _capability_matches
    from app.skills.loader import discover_skills
    from app.config import SKILLS_DIR
    discover_skills(SKILLS_DIR)  # 确保全局注册表已加载全部技能
    assert _capability_matches("", "diagnosis.listening") is True
    assert _capability_matches("diagnosis.listening", "diagnosis.listening") is True
    assert _capability_matches("diagnosis.listening", "diagnosis.inquiry") is False
    assert _capability_matches(
        ["diagnosis.listening", "diagnosis.inquiry"], "diagnosis.inquiry") is True
    assert _capability_matches(
        ["diagnosis.listening", "diagnosis.inquiry"], "treatment.plan") is False
    # 全局注册表确实加载了所有技能且 tcm-rag 工具可被多 capability 检索到
    names = {t.name for t in skill_registry.list_tools()}
    assert {"rag_text_retrieve", "lookup_diet_therapy", "lookup_redflag"}.issubset(names)
    rag_caps = [t.capability for t in skill_registry.list_tools()
                if t.name == "rag_text_retrieve"]
    assert "treatment.plan" in rag_caps[0]
