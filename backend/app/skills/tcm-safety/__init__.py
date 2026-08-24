"""tcm-safety 技能：红旗（red-flag）信号分诊指引库，供「安全」子智能体校准告警。

将识别到的危险信号映射到分级（warning/urgent）、建议就诊科室与处置要点，
使安全 agent 的告警更具可操作性与权威性。本技能仅做分诊提示，不替代急诊判断。
"""
from __future__ import annotations

from app.skills.types import SkillManifest, ToolSpec

# 信号关键词 -> 分诊指引
RED_FLAGS: dict[str, dict] = {
    "胸痛": {"level": "urgent", "where": "急诊 / 胸痛中心", "action": "排查急性冠脉综合征、肺栓塞、主动脉夹层"},
    "咯血": {"level": "urgent", "where": "急诊呼吸科", "action": "排查肺结核、支气管扩张、肺部肿瘤"},
    "呼吸困难": {"level": "urgent", "where": "急诊呼吸科 / 重症", "action": "评估气道、肺与心功能"},
    "意识障碍": {"level": "urgent", "where": "急诊神经内科 / ICU", "action": "立即评估神志与生命体征"},
    "昏迷": {"level": "urgent", "where": "急诊 / ICU", "action": "保持气道通畅，立即急救"},
    "剧烈头痛": {"level": "urgent", "where": "急诊神经内科", "action": "排查脑血管意外、颅内高压"},
    "持续高热": {"level": "warning", "where": "发热门诊 / 感染科", "action": "完善血常规与感染指标"},
    "急性剧烈腹痛": {"level": "urgent", "where": "急诊普外科 / 胃肠外科", "action": "排查急腹症（阑尾炎、穿孔、梗阻）"},
    "呕血": {"level": "urgent", "where": "急诊消化科 / 胃肠外科", "action": "排查上消化道出血"},
    "黑便": {"level": "urgent", "where": "急诊消化科", "action": "警惕上消化道出血"},
    "便血": {"level": "urgent", "where": "急诊肛肠科 / 消化科", "action": "区分上/下消化道出血"},
    "妊娠出血": {"level": "urgent", "where": "急诊产科", "action": "排查流产、异位妊娠等"},
    "体重骤降": {"level": "warning", "where": "全科 / 肿瘤科", "action": "排查消耗性疾病与恶性肿瘤"},
    "自杀": {"level": "urgent", "where": "心理危机干预 / 急救", "action": "立即联系专业援助与亲友陪护"},
    "抽搐": {"level": "urgent", "where": "急诊神经内科", "action": "排查癫痫、惊厥、中毒"},
}


def lookup_redflag(signal: str) -> dict:
    """查询某红旗信号对应的分级、建议就诊科室与处置要点。

    命中不到具体信号时，返回通用就医提示（不漏报）。
    """
    s = (signal or "").strip()
    matches = []
    for key, val in RED_FLAGS.items():
        if s and (s in key or key in s):
            matches.append({"signal": key, **val})
    if matches:
        return {"signal": s, "matched": True, "guidance": matches,
                "note": "以上为分诊提示，最终以线下医疗机构诊断为准。"}
    return {
        "signal": s,
        "matched": False,
        "guidance": [{"signal": "(通用)", "level": "warning",
                      "where": "就近医疗机构", "action": "建议线下面诊进一步评估"}],
        "note": "未命中特定红旗，仍建议就疑似不适线下就医。",
    }


SKILL = SkillManifest(
    name="tcm-safety",
    version="0.1.0",
    description="红旗信号分诊指引技能，供「安全」子智能体将识别到的危险信号映射为分级处置建议。",
    tools=[
        ToolSpec(
            name="lookup_redflag",
            description="查询某危险信号（如胸痛、咯血、意识障碍）对应的分级（warning/urgent）、"
                        "建议就诊科室与处置要点，用于校准安全告警。",
            parameters={
                "type": "object",
                "properties": {
                    "signal": {"type": "string", "description": "识别到的红旗信号关键词"},
                },
                "required": ["signal"],
            },
            capability="diagnosis.safety",
        ),
    ],
)

HANDLERS = {"lookup_redflag": lookup_redflag}
