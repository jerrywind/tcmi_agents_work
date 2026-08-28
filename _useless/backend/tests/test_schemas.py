"""数据模型测试：校验默认值、必填项与字面量约束。"""
import pytest
from pydantic import ValidationError

from app.models.schemas import (
    Consultation, Evidence, Hypothesis, ImageItem, Message, Patient,
    Question, QuestionOption, Report, TreatmentPlan,
)


def test_consultation_defaults():
    c = Consultation(patient=Patient(), complaint="x")
    assert c.status == "created"
    assert c.round == 0
    assert c.evidences == []
    assert c.hypotheses == []
    assert c.trace == []


def test_evidence_and_question():
    e = Evidence(key="k", value="v", source="问", confidence=0.9)
    assert e.round == 0
    q = Question(key="k", text="t", options=[QuestionOption(label="a", value="a")])
    assert q.options[0].value == "a"
    with pytest.raises(ValidationError):
        Question(key="k")  # 缺少 text / options


def test_treatment_plan_category_literal():
    p = TreatmentPlan(id="p1", category="中药方剂", title="t")
    assert p.priority == 1
    assert p.note == ""
    with pytest.raises(ValidationError):
        TreatmentPlan(id="p2", category="无效类别", title="t")


def test_report_optional():
    r = Report()
    assert r.syndromes == []
    assert r.treatments == []
    assert r.red_flag is None


def test_image_item_and_message():
    i = ImageItem(type="tongue", path="/x", url="/u")
    assert i.id
    m = Message(role="user", type="text", content="hi")
    assert m.id


def test_hypothesis_supporting_default():
    h = Hypothesis(name="风寒感冒", confidence=0.6)
    assert h.supporting == []
    assert h.contradicting == []
