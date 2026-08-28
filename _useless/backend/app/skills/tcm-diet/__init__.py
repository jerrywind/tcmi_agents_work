"""tcm-diet 技能：基于证候的中医食疗 / 膳食调护知识检索，供「施治」子智能体调用。

注意：食疗仅为科普性养生参考，涉及具体体质与兼夹证候时须提示由执业中医师辨证指导。
"""
from __future__ import annotations

from app.skills.types import SkillManifest, ToolSpec

# 证候 -> 膳食/食疗建议。foods：宜食；avoid：忌口；rationale：中医机理。
_DIET_THERAPY: dict[str, list[dict]] = {
    "肝郁脾虚": [
        {"title": "疏肝健脾粥", "foods": "山药、莲子、薏米、陈皮、玫瑰花", "avoid": "油腻、生冷、辛辣、酒",
         "rationale": "肝郁乘脾，山药莲子健脾，陈皮玫瑰花疏肝理气"},
        {"title": "情志调食", "foods": "佛手、合欢花代茶饮", "avoid": "咖啡、浓茶等亢奋之品",
         "rationale": "肝喜条达，芳香理气以解郁"},
    ],
    "脾胃湿热": [
        {"title": "清热利湿饮", "foods": "赤小豆、冬瓜、绿豆、薏米", "avoid": "肥甘厚味、甜腻、酒",
         "rationale": "湿热内蕴中焦，淡渗利湿兼以清热"},
        {"title": "忌口要点", "foods": "清淡饮食，少油少盐", "avoid": "烧烤、火锅、芒果榴莲等湿热之品",
         "rationale": "湿热得辛甘厚味则助邪"},
    ],
    "肝胆湿热": [
        {"title": "清肝利胆汤", "foods": "茵陈、栀子（代茶）、芹菜、苦瓜", "avoid": "酒、辛辣、油炸",
         "rationale": "肝胆湿热下注，清利肝胆为要"},
        {"title": "作息调护", "foods": "早睡、规律作息", "avoid": "熬夜、情绪暴怒",
         "rationale": "人卧血归于肝，怒则气上助火"},
    ],
    "心脾两虚": [
        {"title": "补益心脾膳", "foods": "红枣、桂圆、莲子、龙眼肉、小米", "avoid": "生冷、过度思虑耗神",
         "rationale": "气血生化不足，甘温补脾养血安神"},
        {"title": "安神粥", "foods": "酸枣仁（捣碎）煮粥", "avoid": "浓茶、咖啡",
         "rationale": "酸枣仁养心肝之血而安神"},
    ],
    "肾阴虚": [
        {"title": "滋阴补肾膳", "foods": "黑芝麻、桑葚、枸杞、山药、银耳", "avoid": "辛辣燥热、温补壮阳之品",
         "rationale": "肾阴亏虚，甘润滋补以制虚火"},
        {"title": "起居", "foods": "劳逸有度、节欲保精", "avoid": "熬夜、过度劳累",
         "rationale": "房劳熬夜最耗肾阴"},
    ],
    "肺气虚": [
        {"title": "补肺益气膳", "foods": "百合、银耳、山药、太子参、黄芪（少量）", "avoid": "寒凉生冷、过咸",
         "rationale": "肺气不足，甘平益气润肺"},
        {"title": "呼吸调护", "foods": "适度有氧、腹式呼吸", "avoid": "久处烟尘冷空气",
         "rationale": "肺主气司呼吸，温润护卫"},
    ],
    "痰湿蕴肺": [
        {"title": "燥湿化痰膳", "foods": "陈皮、茯苓、薏米、白萝卜", "avoid": "甜腻、生冷、奶制品过量",
         "rationale": "脾为生痰之源，健脾化湿以绝痰源"},
        {"title": "忌口", "foods": "清淡少油", "avoid": "糖果、奶油、冰饮",
         "rationale": "甘助湿、寒凝痰"},
    ],
    "瘀血阻络": [
        {"title": "活血通络膳", "foods": "山楂、黑木耳、桃仁（少量）、玫瑰花", "avoid": "高油高盐、寒凉凝滞",
         "rationale": "血行不畅，辛散温通以活血"},
        {"title": "运动", "foods": "适度舒展运动", "avoid": "久坐不动",
         "rationale": "动则血行，久卧伤气"},
    ],
}

_GENERAL_ADVICE = [
    {"title": "通用膳食原则", "foods": "定时定量、荤素搭配、七分饱、多饮水",
     "avoid": "暴饮暴食、偏食、过饥过饱", "rationale": "脾胃为后天之本，饮食有节则气血生化有源"},
    {"title": "四季调食", "foods": "春增辛甘、夏清淡、秋润燥、冬温补",
     "avoid": "逆时而食", "rationale": "顺应四时以养五脏"},
]


def _match_syndrome(syndrome: str) -> list[dict]:
    s = (syndrome or "").strip()
    if not s:
        return list(_GENERAL_ADVICE)
    # 精确命中
    if s in _DIET_THERAPY:
        return list(_DIET_THERAPY[s])
    # 子串 / 包含匹配
    for key, val in _DIET_THERAPY.items():
        if s in key or key in s:
            return list(val)
    return list(_GENERAL_ADVICE)


def lookup_diet_therapy(syndrome: str) -> dict:
    """根据证候名返回对应的食疗 / 膳食调护建议（宜食、忌口、机理）。

    命中不到具体证候时回退到通用膳食原则。
    """
    items = _match_syndrome(syndrome)
    return {
        "syndrome": syndrome,
        "matched": any(syndrome in _DIET_THERAPY for s in [syndrome]) or syndrome in _DIET_THERAPY,
        "diet_therapy": items,
        "note": "食疗为养生参考，具体体质与兼夹证候须由执业中医师辨证指导。",
    }


SKILL = SkillManifest(
    name="tcm-diet",
    version="0.1.0",
    description="基于证候的中医食疗 / 膳食调护知识检索技能，供「施治」子智能体生成个性化饮食建议。",
    tools=[
        ToolSpec(
            name="lookup_diet_therapy",
            description="查询某证候对应的食疗/膳食调护建议（宜食、忌口、中医机理）。"
                        "用于施治阶段的生活调护建议。命中不到时回退通用原则。",
            parameters={
                "type": "object",
                "properties": {
                    "syndrome": {"type": "string",
                                 "description": "证候名，如 肝郁脾虚 / 脾胃湿热 / 肾阴虚 等"},
                },
                "required": ["syndrome"],
            },
            capability="treatment.plan",
        ),
    ],
)

HANDLERS = {"lookup_diet_therapy": lookup_diet_therapy}
