"""问诊 Sub-Agent：选出"信息增益最大"的下一问。

- rule：对 Top-K 候选证候的特征键做区分度打分，取最高且未问过的键出题。
- llm：由 LLM 在题库范围内选题并润色文案（失败回退规则）。
"""
from __future__ import annotations

from ..knowledge.syndromes import QUESTION_BANK, SYNDROMES
from ..models.schemas import Question, QuestionOption
from ..protocol.base import AgentRequest, AgentResponse, Capability, SubAgent
from ..protocol.llm import get_provider, parse_json
from ..protocol.registry import register
from ..skills.toolcall import run_tool_loop
from app.agents.prompts import system_prompt

TOP_K = 3


def _weight(syndrome: str, key: str) -> float:
    vals = SYNDROMES.get(syndrome, {}).get(key)
    return max(vals.values()) if vals else 0.0


def pick_best_key(req: AgentRequest) -> str | None:
    """区分度 = Top-K 证候两两在该特征上的权重差之和。"""
    top = [h.name for h in req.hypotheses[:TOP_K]] or list(SYNDROMES)[:TOP_K]
    known = {e.key for e in req.evidences} | set(req.asked_keys)
    gender = (req.payload.get("gender") or "未知")
    best_key, best_score = None, 0.0
    for key, meta in QUESTION_BANK.items():
        if key in known:
            continue
        if meta.get("gender") and meta["gender"] != gender:
            continue
        score = 0.0
        for i in range(len(top)):
            for j in range(i + 1, len(top)):
                score += abs(_weight(top[i], key) - _weight(top[j], key))
        score += 0.1 * max(_weight(s, key) for s in top)  # 微弱偏好高权重特征
        if score > best_score:
            best_key, best_score = key, score
    return best_key


def build_question(key: str, text: str | None = None) -> Question:
    meta = QUESTION_BANK[key]
    return Question(
        key=key, text=text or meta["text"],
        options=[QuestionOption(label=o, value=o) for o in meta["options"]],
    )


@register
class InquiryRuleAgent(SubAgent):
    capability = Capability.INQUIRY
    impl_name = "rule"
    description = "信息增益选题（十问歌题库）"

    async def handle(self, req: AgentRequest) -> AgentResponse:
        key = pick_best_key(req)
        if key is None:
            return AgentResponse(capability=self.capability, status="skip",
                                 notes="题库已问尽")
        return AgentResponse(capability=self.capability, question=build_question(key))


@register
class InquiryLLMAgent(SubAgent):
    capability = Capability.INQUIRY
    impl_name = "llm"
    description = "LLM 选题+文案润色（限定题库范围）"

    async def handle(self, req: AgentRequest) -> AgentResponse:
        key = pick_best_key(req)
        if key is None:
            return AgentResponse(capability=self.capability, status="skip",
                                 notes="题库已问尽")
        top = ", ".join(f"{h.name}({h.confidence})" for h in req.hypotheses[:TOP_K])
        prompt = (
            f"你是中医问诊助手。当前候选证候：{top}。"
            f"下一个要询问的特征是「{key}」，默认问题文案：「{QUESTION_BANK[key]['text']}」。"
            "请将文案改写得更亲切自然（保持一个问题、不引入新选项），"
            "仅输出 JSON：{\"text\": str}。"
        )
        raw, usage = await run_tool_loop(
            get_provider(),
            [
                {"role": "system", "content": system_prompt(Capability.INQUIRY)},
                {"role": "user", "content": prompt},
            ],
            req.model, Capability.INQUIRY.value)
        text = parse_json(raw).get("text") or None
        return AgentResponse(capability=self.capability,
                             question=build_question(key, text),
                             meta={"tokens": usage})
