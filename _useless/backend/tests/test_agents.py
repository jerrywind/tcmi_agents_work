"""各 sub-agent 单元测试：rule 与 llm(回退) 两条实现路径。"""
import pytest

from app.models.schemas import Evidence, Hypothesis
from app.protocol.base import Capability

from app.agents.listening import ListeningRuleAgent, extract_keyword_evidences
from app.agents.inspection import InspectionRuleAgent, InspectionVisionAgent
from app.agents.palpation import PalpationRuleAgent
from app.agents.safety import SafetyRuleAgent
from app.agents.inquiry import (
    InquiryLLMAgent, InquiryRuleAgent, build_question, pick_best_key,
)
from app.agents.differentiation import (
    DifferentiationLLMAgent, DifferentiationRuleAgent, score_syndromes,
)
from app.agents.treatment import (
    TreatmentLLMAgent, TreatmentRuleAgent, build_plans, next_question,
)

from app.knowledge.syndromes import QUESTION_BANK, SYNDROMES


def ev(key, value, conf=1.0):
    return Evidence(key=key, value=value, source="闻", confidence=conf)


# ---------------- 闻诊 ----------------
def test_extract_keyword_evidences():
    evs = extract_keyword_evidences("我最近口苦、口臭，大便粘马桶", round_=0, source="闻")
    keys = {e.key: e.value for e in evs}
    assert keys["thirst"] == "口苦"
    assert keys["smell"] == "口臭"
    assert keys["stool"] == "粘滞不爽"


async def test_listening_handle_text(build_req):
    req = build_req(Capability.LISTENING, payload={"text": "我口苦且大便粘"})
    resp = await ListeningRuleAgent().handle(req)
    assert resp.status == "ok"
    keys = {e.key for e in resp.evidences}
    assert "thirst" in keys and "stool" in keys


async def test_listening_handle_empty_text(build_req):
    req = build_req(Capability.LISTENING, payload={"text": ""})
    resp = await ListeningRuleAgent().handle(req)
    assert resp.status == "skip"
    assert resp.evidences == []


async def test_listening_handle_no_input_skips(build_req):
    req = build_req(Capability.LISTENING, payload={})
    resp = await ListeningRuleAgent().handle(req)
    assert resp.status == "skip"


# ---------------- 望诊 ----------------
async def test_inspection_self_report(build_req):
    req = build_req(Capability.INSPECTION,
                    payload={"self_report": {"tongue.body": "红", "tongue.coat": "黄腻"}})
    resp = await InspectionRuleAgent().handle(req)
    assert resp.status == "ok"
    keys = {e.key for e in resp.evidences}
    assert "tongue.body" in keys and "tongue.coat" in keys
    body = next(e for e in resp.evidences if e.key == "tongue.body")
    assert body.confidence == 0.6


async def test_inspection_images_no_report(build_req):
    req = build_req(Capability.INSPECTION,
                    payload={"images": [{"type": "tongue", "path": "x"}]})
    resp = await InspectionRuleAgent().handle(req)
    assert resp.status == "skip"
    assert "已收到" in resp.notes
    assert resp.evidences == []


async def test_inspection_nothing(build_req):
    req = build_req(Capability.INSPECTION, payload={})
    resp = await InspectionRuleAgent().handle(req)
    assert resp.status == "skip"
    assert resp.evidences == []


async def test_inspection_vision_skip_and_fallback(build_req, monkeypatch):
    resp = await InspectionVisionAgent().handle(build_req(Capability.INSPECTION, payload={}))
    assert resp.status == "skip"
    # 无视觉能力（离线 MockProvider）时，提供图片也应优雅 skip，不抛错
    resp2 = await InspectionVisionAgent().handle(
        build_req(Capability.INSPECTION,
                  payload={"images": [{"type": "tongue", "path": "/uploads/x.jpg"}]}))
    assert resp2.status == "skip"
    assert resp2.notes


# ---------------- 切诊 ----------------
async def test_palpation_fast(build_req):
    req = build_req(Capability.PALPATION, payload={"self_report": {"heart_rate": 95}})
    resp = await PalpationRuleAgent().handle(req)
    assert resp.status == "ok"
    assert resp.evidences[0].value == "脉数"


async def test_palpation_slow(build_req):
    req = build_req(Capability.PALPATION, payload={"self_report": {"heart_rate": 50}})
    resp = await PalpationRuleAgent().handle(req)
    assert resp.evidences[0].value == "脉迟"


async def test_palpation_normal(build_req):
    req = build_req(Capability.PALPATION, payload={"self_report": {"heart_rate": 75}})
    resp = await PalpationRuleAgent().handle(req)
    assert "正常" in resp.evidences[0].value


async def test_palpation_nothing(build_req):
    req = build_req(Capability.PALPATION, payload={})
    resp = await PalpationRuleAgent().handle(req)
    assert resp.status == "skip"


# ---------------- 安全红旗 ----------------
async def test_safety_red_flag(build_req):
    req = build_req(Capability.SAFETY, payload={"text": "我突然胸痛放射到左臂"})
    resp = await SafetyRuleAgent().handle(req)
    assert resp.status == "ok"
    assert resp.alerts
    assert any("胸痛" in a.reason for a in resp.alerts)


async def test_safety_clean(build_req):
    req = build_req(Capability.SAFETY, payload={"text": "最近只是有点累"})
    resp = await SafetyRuleAgent().handle(req)
    assert resp.status == "ok"
    assert resp.alerts == []


# ---------------- 问诊（提问） ----------------
async def test_inquiry_asks_question(build_req):
    req = build_req(Capability.INQUIRY,
                    hypotheses=[Hypothesis(name="风寒感冒", confidence=0.3)],
                    asked_keys=[])
    resp = await InquiryRuleAgent().handle(req)
    assert resp.status == "ok"
    assert resp.question is not None
    assert resp.question.options


def test_pick_best_key_male_never_menstruation(build_req):
    for name in SYNDROMES:
        req = build_req(Capability.INQUIRY,
                        hypotheses=[Hypothesis(name=name, confidence=0.3)],
                        payload={"gender": "男"})
        assert pick_best_key(req) != "menstruation"


def test_menstruation_question_is_female_only():
    assert QUESTION_BANK["menstruation"]["gender"] == "女"


async def test_inquiry_llm_fallback(build_req):
    req = build_req(Capability.INQUIRY,
                    hypotheses=[Hypothesis(name="脾胃湿热", confidence=0.3)],
                    asked_keys=[])
    resp = await InquiryLLMAgent().handle(req)
    assert resp.question is not None


def test_build_question_shape():
    q = build_question("chills_fever")
    assert q.key == "chills_fever"
    assert q.options


# ---------------- 辨证 ----------------
def test_score_syndromes_empty():
    hyps = score_syndromes([])
    assert all(h.confidence == 0 for h in hyps)


def test_score_syndromes_top():
    evs = [ev("thirst", "口苦"), ev("smell", "口臭"), ev("stool", "粘滞不爽"),
           ev("head_body", "肢体困重"), ev("appetite", "食欲不振")]
    hyps = score_syndromes(evs)
    assert hyps[0].name == "脾胃湿热"
    assert hyps[0].confidence >= 0.55


def test_score_syndromes_contradiction_lowers_conf():
    # 风寒感冒主证据；再补一条强特征(sweat)但不匹配本证 -> 矛盾减分
    base = [ev("chills_fever", "恶寒重发热轻"), ev("sweat", "无汗")]
    clean = score_syndromes(base)
    contradicted = score_syndromes(base + [ev("sweat", "有汗")])
    top_clean = next(h for h in clean if h.name == "风寒感冒")
    top_bad = next(h for h in contradicted if h.name == "风寒感冒")
    assert top_bad.confidence < top_clean.confidence


async def test_differentiation_rule_returns_hypotheses(build_req):
    evs = [ev("thirst", "口苦"), ev("smell", "口臭"), ev("stool", "粘滞不爽"),
           ev("head_body", "肢体困重"), ev("appetite", "食欲不振")]
    req = build_req(Capability.DIFFERENTIATION, evidences=evs)
    resp = await DifferentiationRuleAgent().handle(req)
    assert resp.hypotheses
    assert resp.hypotheses[0].name == "脾胃湿热"


async def test_differentiation_llm_fallback(build_req):
    evs = [ev("thirst", "口苦"), ev("smell", "口臭")]
    req = build_req(Capability.DIFFERENTIATION, evidences=evs)
    resp = await DifferentiationLLMAgent().handle(req)
    assert isinstance(resp.hypotheses, list)
    assert resp.hypotheses[0].name


# ---------------- 诊疗方案 ----------------
def test_build_plans_order_and_count():
    plans = build_plans(["脾胃湿热"], {})
    assert len(plans) == 5
    assert plans[0].category == "中药方剂"
    priorities = [p.priority for p in plans]
    assert priorities == sorted(priorities)


def test_build_plans_unknown_syndrome():
    assert build_plans(["虚损奇证"], {}) == []


def test_next_question_sequence():
    assert next_question({}, 0, 2).key == "treat.herb_form"
    assert next_question({"treat.herb_form": "x"}, 1, 2).key == "treat.external"
    assert next_question({"treat.herb_form": "x", "treat.external": "y"}, 2, 2) is None
    assert next_question({"treat.herb_form": "x"}, 1, 1) is None


async def test_treatment_rule_asks_first_question(build_req):
    req = build_req(Capability.TREATMENT, payload={"diagnoses": ["脾胃湿热"], "qa": []})
    resp = await TreatmentRuleAgent().handle(req)
    assert resp.question is not None
    assert resp.plans == []


async def test_treatment_rule_rejects_herb(build_req):
    qa = [{"key": "treat.herb_form", "value": "不接受中药"},
          {"key": "treat.external", "value": "接受"}]
    req = build_req(Capability.TREATMENT, payload={"diagnoses": ["脾胃湿热"], "qa": qa})
    resp = await TreatmentRuleAgent().handle(req)
    cats = {p.category for p in resp.plans}
    assert "中药方剂" not in cats
    assert "针灸推拿" in cats


async def test_treatment_rule_granule_note(build_req):
    qa = [{"key": "treat.herb_form", "value": "想要免煎颗粒/中成药"},
          {"key": "treat.external", "value": "接受"}]
    req = build_req(Capability.TREATMENT, payload={"diagnoses": ["脾胃湿热"], "qa": qa})
    resp = await TreatmentRuleAgent().handle(req)
    herb = next(p for p in resp.plans if p.category == "中药方剂")
    assert "免煎" in herb.note


async def test_treatment_rule_pregnancy_note(build_req):
    qa = [{"key": "treat.herb_form", "value": "可煎药"},
          {"key": "treat.pregnancy", "value": "是（孕期/备孕）"}]
    req = build_req(Capability.TREATMENT, payload={"diagnoses": ["脾胃湿热"], "qa": qa})
    resp = await TreatmentRuleAgent().handle(req)
    assert any("孕期" in (p.note or "") for p in resp.plans)


async def test_treatment_rule_empty_diagnoses(build_req):
    qa = [{"key": "treat.herb_form", "value": "可煎药"},
          {"key": "treat.external", "value": "接受"}]
    req = build_req(Capability.TREATMENT, payload={"diagnoses": [], "qa": qa})
    resp = await TreatmentRuleAgent().handle(req)
    assert resp.status == "skip"
    assert resp.plans == []


async def test_treatment_llm_fallback(build_req):
    req = build_req(Capability.TREATMENT, payload={"diagnoses": ["脾胃湿热"], "qa": []})
    resp = await TreatmentLLMAgent().handle(req)
    # MockProvider 返回空 -> 回退到规则方案（5 条）
    assert len(resp.plans) == 5
