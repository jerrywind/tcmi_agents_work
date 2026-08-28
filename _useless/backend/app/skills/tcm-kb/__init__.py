"""内置示例技能：中医知识工具（tcm-kb）。

演示 SKILL 如何为「诊疗方案 / 辨证」LLM 提供可调用工具：
- ``lookup_syndrome_treatment``：按证候查询推荐的多模态诊疗方案；
- ``lookup_herb``：按中药名查询性味归经与功效主治。

该技能在应用启动时由 ``discover_skills`` 自动装载；也可通过
``POST /api/skills/unload`` 与 ``POST /api/skills/load`` 热插拔。

注意：技能模块使用绝对导入（``from app.skills...``），以便装载器通过
importlib 从任意路径加载时相对导入不会失效。
"""
from __future__ import annotations

from app.skills.types import SkillManifest, ToolSpec

# 演示用小型中药库（生产环境建议替换为结构化药库/RAG）
_HERBS: dict[str, dict] = {
    "黄连": {"性味": "苦，寒", "归经": "心、脾、胃、肝、胆、大肠", "功效": "清热燥湿，泻火解毒",
            "主治": "湿热痞满、呕吐、黄疸、高热神昏、心烦不寐、血热吐衄"},
    "黄芩": {"性味": "苦，寒", "归经": "肺、胆、脾、大肠、小肠", "功效": "清热燥湿，泻火解毒，止血，安胎",
            "主治": "肺热咳嗽、湿热泻痢、黄疸、胎动不安"},
    "薏苡仁": {"性味": "甘、淡，凉", "归经": "脾、胃、肺", "功效": "利水渗湿，健脾止泻，除痹，排脓",
              "主治": "水肿、脚气、脾虚泄泻、湿痹拘挛、肺痈、肠痈"},
    "白术": {"性味": "苦、甘，温", "归经": "脾、胃", "功效": "健脾益气，燥湿利水，止汗，安胎",
            "主治": "脾虚食少、腹胀泄泻、痰饮眩悸、水肿、自汗、胎动不安"},
}


SKILL = SkillManifest(
    name="tcm-kb",
    version="0.1.0",
    description="中医知识工具：按证候查询推荐诊疗方案、按中药查询性味功效。供诊疗方案/辨证 LLM 调用。",
    tools=[
        ToolSpec(
            name="lookup_syndrome_treatment",
            description="根据证候名（如 脾胃湿热、风寒感冒）返回推荐的多模态诊疗方案列表"
                        "（中药方剂/针灸推拿/外治法/西医检查/生活调护）。",
            parameters={
                "type": "object",
                "properties": {"syndrome": {"type": "string", "description": "证候名"}},
                "required": ["syndrome"],
            },
            capability="treatment.plan",
        ),
        ToolSpec(
            name="lookup_herb",
            description="查询某味中药的性味、归经、功效与主治。",
            parameters={
                "type": "object",
                "properties": {"herb": {"type": "string", "description": "中药名"}},
                "required": ["herb"],
            },
            capability="treatment.plan",
        ),
    ],
)


async def lookup_syndrome_treatment(syndrome: str) -> dict:
    from app.knowledge.treatments import TREATMENTS

    items = TREATMENTS.get(syndrome)
    if not items:
        return {"found": False, "syndrome": syndrome, "treatments": []}
    return {
        "found": True,
        "syndrome": syndrome,
        "treatments": [
            {
                "category": i.get("category"),
                "title": i.get("title"),
                "detail": i.get("detail"),
                "rationale": i.get("rationale"),
            }
            for i in items
        ],
    }


async def lookup_herb(herb: str) -> dict:
    info = _HERBS.get(herb)
    if not info:
        return {"found": False, "herb": herb}
    return {"found": True, "herb": herb, **info}


HANDLERS = {
    "lookup_syndrome_treatment": lookup_syndrome_treatment,
    "lookup_herb": lookup_herb,
}
