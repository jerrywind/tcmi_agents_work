"""望诊 Sub-Agent：舌象 / 面相 / 患处图像分析。

- rule：无多模态能力，返回 skip，由问诊 agent 引导用户自查舌象（题库含舌质/舌苔题）。
- llm_vision：调用多模态 LLM 输出结构化舌象/面色特征，产出证据。
"""
from __future__ import annotations

from ..models.schemas import Evidence
from ..protocol.base import AgentRequest, AgentResponse, Capability, SubAgent
from ..protocol.llm import get_provider, parse_json
from ..protocol.registry import register
from ..skills.toolcall import run_tool_loop
from app.agents.prompts import system_prompt

_VALID = {
    "tongue.body": ["淡红", "红", "红绛", "淡白", "淡胖有齿痕"],
    "tongue.coat": ["薄白", "薄黄", "黄腻", "白腻", "少苔"],
    "face.color": ["正常", "萎黄少华", "面色晦暗或浮肿", "颧红", "面赤"],
}


@register
class InspectionRuleAgent(SubAgent):
    capability = Capability.INSPECTION
    impl_name = "rule"
    description = "无图像分析能力时的兜底：提示改由问诊引导用户自查"

    async def handle(self, req: AgentRequest) -> AgentResponse:
        evidences: list[Evidence] = []
        # 用户自报舌象（前端可选填），直接采信为低置信度证据
        sr = req.payload.get("self_report") or {}
        for key in ("tongue.body", "tongue.coat"):
            v = sr.get(key)
            if v and v in _VALID[key]:
                evidences.append(Evidence(key=key, value=v, source="望",
                                          confidence=0.6, round=req.round))
        images = req.payload.get("images") or []
        # 按图像类型产出望诊证据；手相（掌纹）列为中医手诊参考维度
        _PALM = {"palm_left": "左", "palm_right": "右"}
        for img in images:
            itype = img.get("type")
            if itype in _PALM:
                evidences.append(Evidence(
                    key=f"望-手{_PALM[itype]}",
                    value=f"已采集{_PALM[itype]}手掌纹",
                    desc=f"已采集{_PALM[itype]}手掌纹/手掌形态，可供中医手诊参考"
                         f"（掌色、掌纹、指形等），结合整体望诊辅助判断气血盛衰与脏腑状态。",
                    source="望", confidence=0.55, round=req.round))
        note = ""
        if images and not evidences:
            note = "已收到照片，当前望诊实现不含图像识别，将通过问诊引导您对照自查舌象。"
        return AgentResponse(capability=self.capability,
                             status="ok" if evidences else "skip",
                             evidences=evidences, notes=note)


@register
class InspectionVisionAgent(SubAgent):
    capability = Capability.INSPECTION
    impl_name = "llm_vision"
    description = "LLM 视觉理解舌象/面象（mmproj 图文理解 + tcm-vision 技能）"

    async def handle(self, req: AgentRequest) -> AgentResponse:
        images = req.payload.get("images") or []
        if not images:
            return AgentResponse(capability=self.capability, status="skip",
                                 notes="未提供图像")
        # 列出图像路径，交由模型按需调用 tcm-vision 技能（analyze_tongue_image 等）获取细节
        path_lines = "\n".join(
            f"- {img.get('type', 'image')}: {img.get('path')}" for img in images
        )
        user = (
            "以下是望诊图像在本服务的本地路径，请按需调用 analyze_tongue_image / analyze_face_image "
            "获取细节，再综合输出结构化望诊结论：\n" + path_lines
        )
        try:
            raw, usage = await run_tool_loop(
                get_provider(),
                [
                    {"role": "system", "content": system_prompt(Capability.INSPECTION)},
                    {"role": "user", "content": user},
                ],
                req.model,
                Capability.INSPECTION.value,
            )
            data = parse_json(raw)
        except Exception:
            return AgentResponse(capability=self.capability, status="skip",
                                 notes="视觉模型不可用，降级为问诊自查",
                                 meta={"degraded": True})
        if not isinstance(data, dict):
            return AgentResponse(capability=self.capability, status="skip",
                                 notes="视觉模型输出异常，降级为问诊自查")
        findings = data.get("findings") or []
        # 仅采纳合法键值对，规避 LLM 臆造
        evidences = []
        for f in findings:
            part = f.get("part")
            value = f.get("value")
            if part in _VALID and value not in _VALID[part]:
                continue
            if part and value:
                evidences.append(Evidence(key=part, value=value, source="望",
                                          confidence=float(f.get("confidence", 0.7)),
                                          round=req.round))
        note = data.get("summary") or ""
        if not evidences:
            return AgentResponse(capability=self.capability, status="skip",
                                 notes=note or "未识别到有效望诊特征，建议自然光下重拍舌象")
        return AgentResponse(capability=self.capability, status="ok",
                             evidences=evidences, notes=note,
                             meta={"tokens": usage})
