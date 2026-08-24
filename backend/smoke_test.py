"""端到端冒烟测试：模拟一次完整问诊 loop（无 LLM，纯规则链路）。"""
import asyncio

from app import agents  # noqa: F401
from app.core.orchestrator import orchestrator
from app.models.schemas import Consultation, Patient

ANSWER_SHEET = {  # 模拟一位"脾胃湿热"用户
    "chills_fever": "无明显寒热",
    "sweat": "正常",
    "thirst": "口苦",
    "stool": "粘滞不爽",
    "urine": "小便短赤",
    "sleep": "正常",
    "appetite": "食欲不振",
    "emotion": "平稳",
    "head_body": "肢体困重",
    "throat_cough": "无",
    "chest_flank": "无",
    "abdomen": "无腹痛",
    "fatigue": "神疲乏力",
    "tongue.body": "红",
    "tongue.coat": "黄腻",
}

TREAT_QA = {  # 诊疗方案个性化追问的答案
    "treat.herb_form": "可煎药",
    "treat.external": "接受",
    "treat.western": "可接受",
    "treat.pregnancy": "否",
}


async def main() -> None:
    c = Consultation(
        patient=Patient(region="广州", height_cm=172, weight_kg=78, age=34, gender="男"),
        complaint="最近总觉得身体沉重，口苦口臭，大便粘马桶，没什么胃口",
        self_report={"heart_rate": 76},
    )
    await orchestrator.start(c)
    rounds = 0
    while c.status in ("waiting_answer", "treatment_qa") and rounds < 30:
        q = c.current_question
        if c.status == "treatment_qa":
            ans = TREAT_QA.get(q.key, q.options[0].value if q.options else "无")
            tag = "TREAT-QA"
        else:
            ans = ANSWER_SHEET.get(q.key, q.options[0].value if q.options else "无")
            tag = f"R{c.round}"
        print(f"[{tag}] Q({q.key}): {q.text} -> {ans}")
        await orchestrator.answer(c, ans, "")
        rounds += 1

    print("\nstatus:", c.status)
    print("evidences:", [(e.key, e.value, e.source) for e in c.evidences])
    print("top3:", [(h.name, h.confidence) for h in c.hypotheses[:3]])
    assert c.report is not None, "no report generated"
    print("\n== 报告 ==")
    print("诊断:", [h.name for h in c.report.syndromes])
    print("依据:", c.report.reasoning)
    print("建议:", c.report.advice)
    print("诊疗方案:")
    for p in c.report.treatments:
        print(f"  [{p.category}] {p.title} | {p.detail[:40]}...")
    print("trace impls:", {t["capability"]: t["impl"] for t in c.trace})
    assert any(h.name == "脾胃湿热" for h in c.report.syndromes), "expected 脾胃湿热"
    assert len(c.report.treatments) > 0, "expected treatment plans"
    print("\nSMOKE TEST PASSED")


asyncio.run(main())
