"""编排器测试：状态机单元方法 + 端到端多轮对话。"""
import pytest

from app.core.orchestrator import Orchestrator, orchestrator
from app.models.schemas import Consultation, Evidence, Hypothesis, Patient, Report
from app.protocol.base import Capability


def _hyp(name, conf=0.7):
    return Hypothesis(name=name, confidence=conf, supporting=[], contradicting=[])


# ---------------- 单元方法 ----------------
def test_pick_final_single(make_consultation):
    c = make_consultation()
    c.evidences = [Evidence(key=f"k{i}", value="v", source="问", confidence=0.9)
                   for i in range(5)]
    c.hypotheses = [_hyp("脾胃湿热", 0.7), _hyp("痰湿困脾", 0.3)]
    final = Orchestrator()._pick_final(c)
    assert final == [c.hypotheses[0]]


def test_pick_final_none_when_few_evidences(make_consultation):
    c = make_consultation()
    c.evidences = []
    c.hypotheses = [_hyp("风寒感冒", 0.9)]
    assert Orchestrator()._pick_final(c) is None


def test_pick_final_dual(make_consultation):
    c = make_consultation()
    c.evidences = [Evidence(key=f"k{i}", value="v", source="问", confidence=0.9)
                   for i in range(6)]
    # 双高且接近 -> 兼证
    c.hypotheses = [_hyp("脾胃湿热", 0.5), _hyp("痰湿困脾", 0.45)]
    final = Orchestrator()._pick_final(c)
    assert len(final) == 2


def test_merge_evidences_override(make_consultation):
    c = make_consultation()
    c.evidences = [Evidence(key="k", value="a", source="问", confidence=0.5)]
    o = Orchestrator()
    o._merge_evidences(c, [Evidence(key="k", value="b", source="望", confidence=0.9)])
    assert c.evidences[0].value == "b"
    o._merge_evidences(c, [Evidence(key="k2", value="c", source="问", confidence=0.9)])
    assert len(c.evidences) == 2


async def test_finish_empty(make_consultation):
    c = make_consultation()
    c.hypotheses = []
    c.evidences = []
    await orchestrator._finish(c, [])
    assert c.report.syndromes == []
    assert "不足以" in c.report.reasoning


async def test_finish_forced(make_consultation):
    c = make_consultation()
    c.evidences = [Evidence(key="k1", value="v", source="问", confidence=0.9)]
    c.hypotheses = [_hyp("风寒感冒", 0.2), _hyp("风热感冒", 0.1)]
    await orchestrator._finish(c, c.hypotheses[:2], forced=True)
    assert c.report is not None
    assert "已达最大问询轮数" in c.report.reasoning


# ---------------- 端到端 ----------------
async def test_treatment_step_flow(make_consultation):
    """辨证已确定时，编排器应进入方案追问并最终生成多模态方案。"""
    c = make_consultation(complaint="x", gender="男")
    c.status = "planning"
    c.hypotheses = [Hypothesis(name="脾胃湿热", confidence=0.7, supporting=[], contradicting=[])]
    c.report = Report(syndromes=list(c.hypotheses))
    await orchestrator._treatment_step(c)
    assert c.status == "treatment_qa"
    assert c.current_question is not None

    q = c.current_question
    c = await orchestrator.answer(c, q.options[0].value, "", sync=True)
    if c.status == "treatment_qa" and c.current_question:
        q2 = c.current_question
        c = await orchestrator.answer(c, q2.options[0].value, "", sync=True)
    assert c.status == "finished"
    assert c.report.treatments
    cats = {t.category for t in c.report.treatments}
    assert "中药方剂" in cats


async def test_treatment_step_rejects_herb(make_consultation):
    c = make_consultation(complaint="x", gender="男")
    c.status = "planning"
    c.hypotheses = [Hypothesis(name="脾胃湿热", confidence=0.7, supporting=[], contradicting=[])]
    c.report = Report(syndromes=list(c.hypotheses))
    await orchestrator._treatment_step(c)
    q = c.current_question  # treat.herb_form
    val = next(o.value for o in q.options if "不接受中药" in o.value)
    c = await orchestrator.answer(c, val, "", sync=True)
    assert c.status == "treatment_qa"
    q2 = c.current_question
    val2 = next(o.value for o in q2.options if o.value == "接受")
    c = await orchestrator.answer(c, val2, "", sync=True)
    assert c.status == "finished"
    cats = {t.category for t in c.report.treatments}
    assert "中药方剂" not in cats
    assert "针灸推拿" in cats


async def test_question_loop_reaches_finished(make_consultation):
    c = make_consultation(complaint="我最近身体不太舒服，有点累", gender="未知")
    c = await orchestrator.start_sync(c)
    steps = 0
    while c.status in ("waiting_answer", "treatment_qa") and steps < 20:
        q = c.current_question
        c = await orchestrator.answer(c, q.options[0].value, "", sync=True)
        steps += 1
    assert c.status == "finished"
    assert c.report is not None


async def test_red_flag_referred(make_consultation):
    c = make_consultation(
        complaint="我突然胸痛，放射到左臂，大汗淋漓", gender="男")
    c = await orchestrator.start_sync(c)
    assert c.status == "referred"
    assert c.report.red_flag


async def test_red_flag_during_treatment_answer(make_consultation):
    c = make_consultation(complaint="x", gender="男")
    c.status = "planning"
    c.hypotheses = [Hypothesis(name="脾胃湿热", confidence=0.7, supporting=[], contradicting=[])]
    c.report = Report(syndromes=list(c.hypotheses))
    await orchestrator._treatment_step(c)  # 进入 treatment_qa
    c = await orchestrator.answer(c, "", "我现在胸痛得厉害", sync=True)
    assert c.status == "referred"


async def test_treatment_step_empty_diagnoses(make_consultation):
    c = make_consultation(gender="男")
    c.status = "planning"
    c.hypotheses = []
    c.report = Report(syndromes=[])
    c.treatment_answers = [{"key": "treat.herb_form", "value": "可煎药"},
                            {"key": "treat.external", "value": "接受"}]
    await orchestrator._treatment_step(c)
    assert c.status == "finished"
    assert c.report.treatments == []
