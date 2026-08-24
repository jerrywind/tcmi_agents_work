"""用药安全硬校验层：十八反 / 十九畏 / 孕忌 / 体质禁忌。

在施治 Agent 产出方剂后、写入报告前运行，作为合规兜底（规则优先，不依赖 LLM）。
方剂组成从 TreatmentPlan.detail 解析（顿号/逗号/空格分隔）。
"""
from __future__ import annotations

import re

# 十八反：配伍相反（不宜同用）
EIGHT_INCOMPATIBILITIES: dict[str, list[str]] = {
    "甘草": ["海藻", "大戟", "甘遂", "芫花"],
    "乌头": ["半夏", "瓜蒌", "贝母", "白蔹", "白及"],
    "藜芦": ["人参", "沙参", "丹参", "玄参", "细辛", "芍药"],
}

# 十九畏：配伍相畏（不宜同用）
NINETEEN_ANTIAGONISMS: list[tuple[str, str]] = [
    ("硫黄", "朴硝"), ("水银", "砒霜"), ("狼毒", "密陀僧"),
    ("巴豆", "牵牛"), ("丁香", "郁金"), ("川乌/草乌", "犀角"),
    ("牙硝", "三棱"), ("官桂", "石脂"),
]

# 孕忌药材（妊娠禁忌，峻下/活血/有毒类）
PREGNANCY_CONTRAINDICATED: set[str] = {
    "巴豆", "牵牛", "大戟", "甘遂", "芫花", "麝香", "三棱", "莪术",
    "水蛭", "虻虫", "斑蝥", "附子", "桃仁", "红花", "牛膝", "丹参",
    "川芎", "当归尾", "瞿麦", "通草",
}

# 体质禁忌：热证/阴虚忌温燥，寒证忌寒凉
CONSTITUTION_NOTE: dict[str, set[str]] = {
    "热": {"干姜", "吴茱萸", "附子", "肉桂", "桂枝", "细辛", "麻黄"},
    "寒": {"黄连", "黄柏", "知母", "石膏", "黄芩", "龙胆草", "苦参"},
}


def _extract_herbs(detail: str) -> list[str]:
    """从方剂组成文本中提取药材名（去剂量/炮制说明）。"""
    if not detail:
        return []
    # 去掉括号内的说明，如 "薄荷(后下)"、"吴茱萸研末"
    detail = re.sub(r"[（(][^）)]*[）)]", "", detail)
    # 按顿号、逗号、分号、空格切分
    parts = re.split(r"[、，,；; ]+", detail)
    herbs: list[str] = []
    for p in parts:
        p = p.strip()
        # 去掉 "水煎服" 等非药材词及尾随 "。" 等
        p = re.sub(r"[。\.].*$", "", p)
        if p and len(p) <= 4 and p not in ("水煎服", "日一剂", "温服", "成药"):
            herbs.append(p)
    return herbs


def check_plan_safety(detail: str, *, pregnant: bool = False,
                      constitution: str = "") -> list[str]:
    """返回该方剂的安全警示列表（空表示无明显冲突）。"""
    warnings: list[str] = []
    herbs = _extract_herbs(detail)
    herb_set = set(herbs)

    # 十八反
    for a, conflicts in EIGHT_INCOMPATIBILITIES.items():
        hit = sorted(herb_set & set(conflicts))
        if a in herb_set and hit:
            warnings.append(f"配伍禁忌（十八反）：{a} 不宜与 {'、'.join(hit)} 同用。")

    # 十九畏（双向）
    for x, y in NINETEEN_ANTIAGONISMS:
        if x in herb_set and y in herb_set:
            warnings.append(f"配伍禁忌（十九畏）：{x} 与 {y} 相畏，不宜同用。")

    # 孕忌
    if pregnant:
        hit = sorted(herb_set & PREGNANCY_CONTRAINDICATED)
        if hit:
            warnings.append(
                f"妊娠禁忌：{'、'.join(hit)} 属妊娠慎用/禁用药，孕期/备孕须医师指导下使用。")

    # 体质禁忌提示
    if constitution in CONSTITUTION_NOTE:
        hit = sorted(herb_set & CONSTITUTION_NOTE[constitution])
        if hit:
            warnings.append(
                f"体质提示：辨为{constitution}证倾向，{'、'.join(hit)} 偏温燥/寒凉，"
                f"建议医师据证微调。")

    return warnings


# 暴露给调用方便于识别"安全闸门提示"前缀（前端可据此高亮）
SAFETY_PREFIX = "⚠ 用药安全提示："
