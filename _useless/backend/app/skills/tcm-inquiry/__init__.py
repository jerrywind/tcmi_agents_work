"""tcm-inquiry 技能：问诊追问聚焦，供「问诊」子智能体选择下一个最具鉴别力的问题。

基于知识库 SYNDROMES（证候 -> 特征权重）与 QUESTION_BANK（特征 -> 问题文案/选项），
既能按候选证候给出应重点追问的特征，也能根据已有症状反推最可能证候并建议下一步追问。
"""
from __future__ import annotations

from app.knowledge.syndromes import QUESTION_BANK, SYNDROMES
from app.skills.types import SkillManifest, ToolSpec

_TOP_N = 3


def _focus_for(syndrome: str) -> list[dict]:
    feats = SYNDROMES.get(syndrome, {})
    # 按权重降序取最具鉴别力的特征
    ranked = sorted(feats.items(), key=lambda kv: max(kv[1].values()), reverse=True)[:_TOP_N]
    out = []
    for key, value_weights in ranked:
        meta = QUESTION_BANK.get(key)
        out.append({
            "feature_key": key,
            "question": meta["text"] if meta else key,
            "options": meta.get("options", []) if meta else [],
            "top_values": sorted(value_weights, key=lambda k: value_weights.get(k, 0.0), reverse=True),
        })
    return out


def lookup_inquiry_focus(syndrome: str) -> dict:
    """返回某候选证候最值得追问的特征（问题文案、选项、典型取值）。"""
    return {"syndrome": syndrome, "focus": _focus_for(syndrome),
            "note": "优先追问权重高、且尚未采集到的特征。"}


def suggest_followup(symptoms: str) -> dict:
    """根据已采集的症状描述，反推最可能证候，并建议下一个最具鉴别力的追问。

    也把命中的候选证候列出，供问诊 agent 收敛方向。
    """
    text = (symptoms or "").lower()
    # 用 SYNDROMES 的特征取值做 症状 -> 证候 匹配，统计命中特征数
    scored: dict[str, float] = {}
    for syn, feats in SYNDROMES.items():
        score = 0.0
        for vals in feats.values():
            for v in vals:
                if v and v.lower() in text:
                    score += 1.0
        if score > 0:
            scored[syn] = score
    candidates = sorted(scored.items(), key=lambda x: x[1], reverse=True)[:_TOP_N]

    next_q = None
    if candidates:
        top_syn = candidates[0][0]
        for f in _focus_for(top_syn):
            # 该特征仍有未采集到的典型取值，则建议追问该项并给出缺漏取值
            missing = [v for v in f["top_values"] if v.lower() not in text]
            if missing:
                next_q = {"syndrome": top_syn, "feature_key": f["feature_key"],
                          "question": f["question"], "options": f["options"],
                          "suggested_value": missing[0]}
                break

    return {
        "symptoms": symptoms,
        "candidate_syndromes": [{"syndrome": s, "score": sc} for s, sc in candidates],
        "suggested_next_question": next_q,
        "note": "命中不到具体证候时，请基于四诊与开放提问继续采集。",
    }


SKILL = SkillManifest(
    name="tcm-inquiry",
    version="0.1.0",
    description="问诊追问聚焦技能，基于证候特征权重与题库，供「问诊」子智能体选择下一个最具鉴别力的问题。",
    tools=[
        ToolSpec(
            name="lookup_inquiry_focus",
            description="给定候选证候，返回最值得追问的特征（问题文案、选项、典型取值），"
                        "帮助问诊 agent 收敛提问方向。",
            parameters={
                "type": "object",
                "properties": {
                    "syndrome": {"type": "string", "description": "候选证候名，如 脾胃湿热 / 肝郁气滞"},
                },
                "required": ["syndrome"],
            },
            capability="diagnosis.inquiry",
        ),
        ToolSpec(
            name="suggest_followup",
            description="根据已采集的症状描述，反推最可能证候并建议下一个最具鉴别力的追问问题，"
                        "同时列出候选证候供问诊收敛。",
            parameters={
                "type": "object",
                "properties": {
                    "symptoms": {"type": "string", "description": "当前已采集的症状/证据描述文本"},
                },
                "required": ["symptoms"],
            },
            capability="diagnosis.inquiry",
        ),
    ],
)

HANDLERS = {"lookup_inquiry_focus": lookup_inquiry_focus, "suggest_followup": suggest_followup}
