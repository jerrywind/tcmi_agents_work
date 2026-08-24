"""tcm-palpation 技能：切诊（脉象 / 腹诊）术语参照库，供「切诊」子智能体校准证据表述。

将用户自述的脉感、腹诊、肢体温度等口语映射到标准中医术语与病机。
"""
from __future__ import annotations

from app.skills.types import SkillManifest, ToolSpec

# 脉象术语 -> 主病/含义
PULSE_PATTERNS: dict[str, str] = {
    "浮脉": "邪在表，外邪袭表，脉位表浅",
    "沉脉": "病在里，邪郁于内",
    "迟脉": "寒证（脉率慢，一息不足四至）",
    "数脉": "热证（脉率快，一息五至以上）",
    "虚脉": "气血不足，脉来无力",
    "实脉": "邪气盛实，脉来有力",
    "弦脉": "肝胆病、痛证、痰饮，端直而长如按琴弦",
    "滑脉": "痰饮、食滞、实热，往来流利（妊娠亦可见）",
    "细脉": "气血两虚、湿阻，脉细如线",
    "涩脉": "血行不畅、气滞血瘀，往来艰涩",
    "洪脉": "热盛，脉大而充实有力",
    "濡脉": "虚证夹湿，浮细而软",
    "结代脉": "气血亏虚、心阳不振，心律不齐（需警惕心系疾患）",
    "紧脉": "寒证、痛证、宿食，脉来紧张",
}

# 腹诊 / 触诊术语 -> 含义
ABDOMEN_PATTERNS: dict[str, str] = {
    "腹软喜按": "虚证，按之痛减",
    "腹痛拒按": "实证、里实积滞，按之痛增",
    "腹中结块": "癥瘕积聚，需详辨良恶与部位",
    "少腹急结": "热结膀胱或蓄血，小腹拘急硬满",
    "四肢不温": "阳气虚衰或寒凝",
    "手足心热": "阴虚内热",
    "肌肤甲错": "瘀血内停，肌肤失养",
}


def _search(patterns: dict[str, str], query: str) -> list[dict]:
    q = (query or "").strip()
    if not q:
        return []
    qchars = set(q)
    hits = []
    for k, v in patterns.items():
        # 1) 精确包含
        if q in k or k in q:
            hits.append({"term": k, "meaning": v})
            continue
        # 2) 字符重叠：应对口语/语序差异
        kchars = set(k)
        if qchars and kchars and len(qchars & kchars) / max(1, len(qchars)) >= 0.5:
            hits.append({"term": k, "meaning": v})
    return hits


def lookup_pulse_pattern(query: str) -> dict:
    """检索与脉象相关的标准中医术语及其主病/含义。"""
    return {"category": "pulse", "query": query, "matches": _search(PULSE_PATTERNS, query)}


def lookup_abdomen_pattern(query: str) -> dict:
    """检索与腹诊 / 触诊（腹诊、四肢温度、肌肤）相关的标准术语及含义。"""
    return {"category": "abdomen", "query": query, "matches": _search(ABDOMEN_PATTERNS, query)}


SKILL = SkillManifest(
    name="tcm-palpation",
    version="0.1.0",
    description="切诊（脉象、腹诊、触诊）标准术语与病机参照技能，供「切诊」子智能体校准证据。",
    tools=[
        ToolSpec(
            name="lookup_pulse_pattern",
            description="检索与脉象相关的标准中医术语及其主病/含义，"
                        "用于把脉率/脉感的口语描述映射到规范证据取值。",
            parameters={
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "脉相关描述，如 脉细/脉弦/脉快"},
                },
                "required": ["query"],
            },
            capability="diagnosis.palpation",
        ),
        ToolSpec(
            name="lookup_abdomen_pattern",
            description="检索与腹诊/触诊（腹软喜按、四肢温度、肌肤等）相关的标准术语及含义。",
            parameters={
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "腹诊/触诊描述，如 肚子怕按/手脚凉"},
                },
                "required": ["query"],
            },
            capability="diagnosis.palpation",
        ),
    ],
)

HANDLERS = {"lookup_pulse_pattern": lookup_pulse_pattern,
            "lookup_abdomen_pattern": lookup_abdomen_pattern}
