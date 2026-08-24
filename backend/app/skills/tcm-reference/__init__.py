"""内置技能：中医证候典型表现检索（tcm-reference）。

供「辨证」LLM（diagnosis.differentiation）按需调用，查询某证候的典型四诊表现，
以校准候选证候与支撑证据，减少 LLM 臆造证候特征。
"""
from __future__ import annotations

from app.knowledge.syndromes import SYNDROMES
from app.skills.types import SkillManifest, ToolSpec

# 反向索引：证候 -> [{"category":..,"value":..}, ...]
_SYNDROME_PATTERNS: dict[str, list[dict]] = {}
for _syn, _features in SYNDROMES.items():
    for _cat, _vals in _features.items():
        for _val in _vals:
            _SYNDROME_PATTERNS.setdefault(_syn, []).append({"category": _cat, "value": _val})


def _find(syndrome: str) -> tuple[str | None, list | dict]:
    if syndrome in _SYNDROME_PATTERNS:
        return syndrome, _SYNDROME_PATTERNS[syndrome]
    matches = [s for s in _SYNDROME_PATTERNS if syndrome in s]
    if len(matches) == 1:
        return matches[0], _SYNDROME_PATTERNS[matches[0]]
    # 多匹配或无匹配都返回候选列表，交由模型进一步指定
    return None, matches


async def lookup_syndrome_patterns(syndrome: str) -> dict:
    """查询某证候的典型四诊表现（category/value 列表）。"""
    if not syndrome or not syndrome.strip():
        return {"found": False, "error": "缺少 syndrome 参数"}
    name, pats = _find(syndrome.strip())
    if name is None and isinstance(pats, list) and pats and isinstance(pats[0], str):
        return {"found": False, "candidates": pats,
                "note": "匹配到多个证候，请指定更精确名称"}
    if name is None:
        return {"found": False, "syndrome": syndrome, "patterns": []}
    return {"found": True, "syndrome": name, "patterns": pats}


SKILL = SkillManifest(
    name="tcm-reference",
    version="0.1.0",
    description="中医证候典型表现检索：按证候名返回典型四诊表现，辅助辨证校准。供辨证 LLM 调用。",
    tools=[
        ToolSpec(
            name="lookup_syndrome_patterns",
            description="根据证候名（如 肝郁脾虚、脾胃湿热）返回其典型四诊表现列表 "
                        "（每项含 category/value），用于校验辨证结论是否符合该证典型特征。",
            parameters={
                "type": "object",
                "properties": {"syndrome": {"type": "string", "description": "证候名"}},
                "required": ["syndrome"],
            },
            capability="diagnosis.differentiation",
        ),
    ],
)

HANDLERS = {"lookup_syndrome_patterns": lookup_syndrome_patterns}
