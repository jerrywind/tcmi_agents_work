"""tcm-auscultation 技能：闻诊（声/嗅）术语参照库，供「闻诊」子智能体校准证据表述。

将口语化描述映射到标准中医术语与病机，帮助 agent 输出一致、可解释的证据 value。
"""
from __future__ import annotations

from app.skills.types import SkillManifest, ToolSpec

# 语声 / 呼吸异常术语 -> 病机含义
VOICE_PATTERNS: dict[str, str] = {
    "声高息粗": "实证、热证，肺气壅实或痰热内盛",
    "声低息微": "虚证、寒证，气虚不足或久病正气耗伤",
    "语声重浊": "外感风寒或湿浊中阻，肺气不宣",
    "语言謇涩": "风痰阻络，常见于中风先兆或后遗症",
    "郑声": "神识不清、语言重复、声低无力，正气大伤（危重症象）",
    "谵语": "神识不清、语无伦次、声高有力，实热扰心（危重症象）",
    "独语": "心气不足或气郁痰结，常见于癫证",
    "狂言": "痰火扰心，常见于狂证",
    "喷嚏": "风寒外袭，肺气上逆",
    "太息": "情志不遂，肝气郁结",
}

# 气味异常术语 -> 病机含义
ODOR_PATTERNS: dict[str, str] = {
    "口气酸臭": "食积胃肠，腐熟之乡",
    "口气腐臭": "胃热或牙疳",
    "汗气腥膻": "湿热蕴蒸肌肤",
    "痰涕腥臭": "热毒壅肺，需警惕肺痈",
    "二便臭秽": "湿热下注或肠腑积热",
    "体气臊臭": "下焦湿热或肾虚心火",
    "白带腥臭": "湿热下注带脉",
}

# 咳嗽特征术语 -> 病机含义
COUGH_PATTERNS: dict[str, str] = {
    "咳声重浊": "外感风寒或痰湿壅肺",
    "咳声清脆": "燥邪犯肺或阴虚肺燥",
    "干咳无痰": "肺阴亏虚或燥邪伤津",
    "咳声不扬": "热邪壅肺，肺气不宣",
    "犬吠样咳": "疫毒攻喉（如喉炎），需警惕气道梗阻",
    "阵发痉挛性咳": "顿咳（百日咳）特征",
}


def _search(patterns: dict[str, str], query: str) -> list[dict]:
    q = (query or "").strip()
    if not q:
        return []
    qchars = set(q)
    hits = []
    for k, v in patterns.items():
        # 1) 精确包含：查询是术语子串或术语是查询子串
        if q in k or k in q:
            hits.append({"term": k, "meaning": v})
            continue
        # 2) 字符重叠：查询中过半汉字命中术语（应对口语/语序差异）
        kchars = set(k)
        if qchars and kchars and len(qchars & kchars) / max(1, len(qchars)) >= 0.5:
            hits.append({"term": k, "meaning": v})
    return hits


def lookup_voice_pattern(query: str) -> dict:
    """在闻诊术语库中检索与「语声/呼吸」相关的标准术语与病机含义。"""
    return {"category": "voice/breathing", "query": query,
            "matches": _search(VOICE_PATTERNS, query) + _search(COUGH_PATTERNS, query)}


def lookup_odor_pattern(query: str) -> dict:
    """在闻诊术语库中检索与「气味」相关的标准术语与病机含义。"""
    return {"category": "odor", "query": query, "matches": _search(ODOR_PATTERNS, query)}


SKILL = SkillManifest(
    name="tcm-auscultation",
    version="0.1.0",
    description="闻诊（语声、呼吸、咳嗽、气味）标准术语与病机参照技能，供「闻诊」子智能体校准证据。",
    tools=[
        ToolSpec(
            name="lookup_voice_pattern",
            description="检索与语声/呼吸/咳嗽相关的标准中医术语及其病机含义，"
                        "用于把口语化描述映射到规范证据取值。",
            parameters={
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "用户描述或候选术语，如 声音低/喘/咳声重"},
                },
                "required": ["query"],
            },
            capability="diagnosis.listening",
        ),
        ToolSpec(
            name="lookup_odor_pattern",
            description="检索与气味（口气/体气/排泄物）相关的标准中医术语及其病机含义。",
            parameters={
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "气味相关描述，如 口臭/汗腥"},
                },
                "required": ["query"],
            },
            capability="diagnosis.listening",
        ),
    ],
)

HANDLERS = {"lookup_voice_pattern": lookup_voice_pattern, "lookup_odor_pattern": lookup_odor_pattern}
