"""知识库测试：证候库、问诊题库、方案库与红旗词的健全性校验。"""
from app.knowledge.syndromes import (
    ADVICE, KEYWORD_EVIDENCE, RED_FLAGS, SYNDROMES, QUESTION_BANK,
    feature_keys_of,
)
from app.knowledge.treatments import (
    TREATMENT_QUESTION_ORDER, TREATMENT_QUESTIONS, TREATMENTS,
)

SYNDROME_NAMES = list(SYNDROMES.keys())


def test_syndromes_complete():
    assert len(SYNDROME_NAMES) == 8
    for name, features in SYNDROMES.items():
        assert isinstance(features, dict)
        for fk, values in features.items():
            assert isinstance(values, dict)  # {特征值: 权重}


def test_feature_keys_of():
    keys = feature_keys_of(["风寒感冒"])
    assert "chills_fever" in keys
    assert "sweat" in keys
    assert feature_keys_of([]) == []


def test_question_bank_well_formed():
    assert QUESTION_BANK
    for key, bank in QUESTION_BANK.items():
        assert "text" in bank and bank["text"]
        assert "options" in bank and bank["options"]
        for opt in bank["options"]:
            assert isinstance(opt, str) and opt


def test_advice_covers_syndromes():
    for name in SYNDROME_NAMES:
        assert name in ADVICE, f"{name} 缺调护建议"


def test_red_flags_present():
    keywords = [kw for kw, _ in RED_FLAGS]
    assert "胸痛" in keywords
    assert "便血" in keywords


def test_keyword_evidence_mapping():
    entries = {(kws, key, val) for kws, key, val in KEYWORD_EVIDENCE}
    assert (("口苦",), "thirst", "口苦") in entries
    assert (("大便粘", "粘马桶"), "stool", "粘滞不爽") in entries
    assert (("没胃口", "食欲差", "不想吃"), "appetite", "食欲不振") in entries


def test_treatments_complete():
    assert set(TREATMENTS.keys()) == set(SYNDROME_NAMES)


def test_treatment_questions_sorted_by_priority():
    orders = TREATMENT_QUESTION_ORDER
    assert set(orders) == set(TREATMENT_QUESTIONS.keys())
    priorities = [TREATMENT_QUESTIONS[k]["priority"] for k in orders]
    assert priorities == sorted(priorities)


def test_treatment_questions_well_formed():
    for key, q in TREATMENT_QUESTIONS.items():
        assert q.get("text")
        assert q.get("options")
        for opt in q["options"]:
            assert isinstance(opt, str) and opt
