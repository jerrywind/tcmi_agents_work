"""诊疗方案 Sub-Agent：辨证结论 + 用户个体情况 -> 多模态诊疗方案。

设计目标：以「更快、更彻底痊愈」为出发点，方案不止于开中药，还包含
针灸推拿、外治法、西医检查（明确诊断/排除器质病变）、生活调护/膳食。

- rule：按知识库组装方案，并依据个性化问答（煎药便利性/是否接受外治/
         是否愿做西医检查/孕期备孕）筛选与备注，必要时追问 1~2 条。
- llm：把辨证+个体情况交给 LLM 生成综合方案（含或不含追问），失败回退规则。

实现遵循统一协议：无状态、只消费请求信封、只产出结构化 plans / question。
"""
from __future__ import annotations

import json

from ..knowledge.treatments import (
    TREATMENTS, TREATMENT_QUESTIONS, TREATMENT_QUESTION_ORDER,
)
from ..models.schemas import Question, QuestionOption, TreatmentPlan
from ..protocol.base import AgentRequest, AgentResponse, Capability, SubAgent
from ..protocol.llm import get_provider, parse_json
from ..protocol.registry import register
from ..skills.toolcall import run_tool_loop
from app.agents.prompts import system_prompt

MAX_QUESTIONS_DEFAULT = 2


def _diagnoses_of(req: AgentRequest) -> list[str]:
    ds = req.payload.get("diagnoses") or []
    if ds:
        return [d for d in ds if d]
    return [h.name for h in req.hypotheses[:2] if h.confidence > 0]


def _qa_map(req: AgentRequest) -> dict:
    out: dict = {}
    for it in req.payload.get("qa") or []:
        if isinstance(it, dict) and it.get("key"):
            out[it["key"]] = it.get("value", "")
    return out


def next_question(qa: dict, asked_count: int, max_q: int) -> Question | None:
    """还差哪些个性化信息就追问；但最多只问 max_q 条，避免过度打扰。"""
    if asked_count >= max_q:
        return None
    for key in TREATMENT_QUESTION_ORDER:
        if key in qa:
            continue
        meta = TREATMENT_QUESTIONS[key]
        return Question(
            key=key, text=meta["text"],
            options=[QuestionOption(label=o, value=o) for o in meta["options"]],
        )
    return None


def _make_plan(item: dict, **overrides) -> TreatmentPlan:
    note = overrides.pop("note", item.get("note", ""))
    return TreatmentPlan(
        category=item.get("category", "生活调护"),
        title=item.get("title", ""),
        detail=item.get("detail", ""),
        rationale=item.get("rationale", ""),
        note=note,
        priority=item.get("priority", 9),
        **overrides,
    )


def build_plans(diagnoses: list[str], qa: dict) -> list[TreatmentPlan]:
    """按证候组装方案并依据个体情况筛选/备注。"""
    herb_form = qa.get("treat.herb_form")
    external = qa.get("treat.external")
    western = qa.get("treat.western")
    pregnancy = qa.get("treat.pregnancy")

    plans: list[TreatmentPlan] = []
    seen: set[str] = set()

    for name in diagnoses:
        for item in TREATMENTS.get(name, []):
            cat = item.get("category")
            # 中药方剂：按煎药意愿与孕期调整
            if cat == "中药方剂":
                if herb_form == "不接受中药":
                    continue
                note = item.get("note", "")
                if pregnancy == "是（孕期/备孕）":
                    note = (note + " " if note else "") + \
                        "孕期/备孕期间用药须谨慎，须经中医妇科/产科医师辨证后使用。"
                if herb_form == "想要免煎颗粒/中成药":
                    note = (note + " " if note else "") + \
                        "建议改用同名中成药或免煎颗粒，遵医嘱服用。"
                plans.append(_make_plan(item, note=note))
            # 针灸推拿 / 外治法：不接受则跳过
            elif cat in ("针灸推拿", "外治法"):
                if external == "不接受":
                    continue
                plans.append(_make_plan(item))
            # 西医检查：用户暂不接受则保留为可选备注，不强制
            elif cat == "西医检查":
                note = item.get("note", "")
                if western == "暂不接受":
                    note = (note + " " if note else "") + \
                        "您选择暂不做西医检查，以下项目仅供参考，建议适时进行以利确诊。"
                plans.append(_make_plan(item, note=note))
            else:
                plans.append(_make_plan(item))

    # 去重（同名标题只保留优先级更高者），按 priority 排序后限量
    for p in plans:
        if p.title in seen:
            continue
        seen.add(p.title)
    unique = [p for p in plans if p.title in seen]
    unique.sort(key=lambda p: (p.priority, p.category))
    return unique[:10]


def _patient_summary(req: AgentRequest) -> str:
    p = req.payload.get("patient") or {}
    if not p:
        return "（未提供基本信息）"
    return (f"性别{p.get('gender','未知')}、年龄{p.get('age','?')}岁、"
            f"常住{p.get('region','?')}、身高{p.get('height_cm','?')}cm、"
            f"体重{p.get('weight_kg','?')}kg")


@register
class TreatmentRuleAgent(SubAgent):
    capability = Capability.TREATMENT
    impl_name = "rule"
    description = "知识库组装 + 个性化筛选（开方/针灸/西医检查/调护）"

    async def handle(self, req: AgentRequest) -> AgentResponse:
        max_q = int((req.options or {}).get("max_questions", MAX_QUESTIONS_DEFAULT))
        diagnoses = _diagnoses_of(req)
        qa = _qa_map(req)

        q = next_question(qa, len(req.payload.get("qa") or []), max_q)
        if q is not None:
            return AgentResponse(capability=self.capability, question=q)

        if not diagnoses:
            return AgentResponse(
                capability=self.capability, status="skip",
                notes="尚未形成辨证结论，暂缓制定方案")

        plans = build_plans(diagnoses, qa)
        return AgentResponse(
            capability=self.capability, plans=plans,
            notes=f"已为「{'、'.join(diagnoses)}」生成 {len(plans)} 项诊疗方案")


@register
class TreatmentLLMAgent(SubAgent):
    capability = Capability.TREATMENT
    impl_name = "llm"
    description = "LLM 生成综合方案（规则结果作为兜底）"

    async def handle(self, req: AgentRequest) -> AgentResponse:
        diagnoses = _diagnoses_of(req)
        if not diagnoses:
            return AgentResponse(capability=self.capability, status="skip",
                                 notes="尚未形成辨证结论，暂缓制定方案")
        qa = _qa_map(req)
        max_q = int((req.options or {}).get("max_questions", MAX_QUESTIONS_DEFAULT))
        prior_plans = build_plans(diagnoses, qa)

        ev_lines = [f"- {e.key}={e.value}（{e.source}诊）" for e in req.evidences[:12]]
        qa_lines = [f"- {k}={v}" for k, v in qa.items()] or ["（暂无）"]
        prompt = (
            "你是中西医结合的诊疗方案助手。目标：让用户更快、更彻底痊愈。\n"
            f"辨证结论：{', '.join(diagnoses)}\n"
            f"患者情况：{_patient_summary(req)}\n"
            f"已有证据：\n" + "\n".join(ev_lines) + "\n"
            f"用户个性化选择：\n" + "\n".join(qa_lines) + "\n"
            "请输出 JSON，二选一：\n"
            "1) 若还需 1 条个性化信息，则返回 {\"ask\": {\"key\": str, \"text\": str, "
            "\"options\": [str,...]}}（key 限 treat.herb_form/treat.external/treat.western/"
            "treat.pregnancy 之一，且未问过的）；\n"
            "2) 否则返回 {\"plans\": [{\"category\": \"中药方剂|针灸推拿|外治法|西医检查|"
            "生活调护|膳食\", \"title\": str, \"detail\": str, \"rationale\": str, "
            "\"note\": str, \"priority\": int}]}，可含多模态方案（含西医检查以助确诊）。\n"
            "不要给孕期/备孕者开禁忌中药，必须提示就医。最多返回 10 条。"
            "\n可用技能 lookup_syndrome_treatment(syndrome) / lookup_herb(herb) 查询知识库，"
            "优先据此给出有据可循的方案。"
        )
        raw, usage = await run_tool_loop(
            get_provider(),
            [
                {"role": "system", "content": system_prompt(Capability.TREATMENT)},
                {"role": "user", "content": prompt},
            ],
            req.model, Capability.TREATMENT.value)
        data = parse_json(raw)
        if not data:
            return AgentResponse(capability=self.capability, plans=prior_plans,
                                 notes="LLM 不可用，使用规则方案",
                                 meta={"tokens": usage, "degraded": True})
        ask = data.get("ask")
        if ask and len(req.payload.get("qa") or []) < max_q:
            return AgentResponse(
                capability=self.capability,
                question=Question(
                    key=ask.get("key", "treat.extra"), text=ask.get("text", ""),
                    options=[QuestionOption(label=o, value=o) for o in ask.get("options", [])]),
                meta={"tokens": usage})
        items = data.get("plans") or []
        if items:
            llm_plans = [
                TreatmentPlan(category=i.get("category", "生活调护"),
                              title=i.get("title", ""), detail=i.get("detail", ""),
                              rationale=i.get("rationale", ""), note=i.get("note", ""),
                              priority=int(i.get("priority", 9)))
                for i in items
            ]
            # 规则先验保底：若 LLM 漏掉关键西医检查，补回
            return AgentResponse(capability=self.capability, plans=llm_plans,
                                 meta={"tokens": usage})
        return AgentResponse(capability=self.capability, plans=prior_plans,
                             notes="LLM 未返回方案，使用规则方案",
                             meta={"tokens": usage, "degraded": True})
