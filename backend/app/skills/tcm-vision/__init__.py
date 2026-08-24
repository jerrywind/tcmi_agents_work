"""内置技能：中医望诊多模态（tcm-vision）。

借助 llm_server 中独立部署的 Qwen3-VL 视觉模型（原生多模态，无需 mmproj），
分析舌象/面象图像，产出客观望诊观察。
供「望诊」LLM（diagnosis.inspection）按需调用：模型可决定对哪张图片调用
analyze_tongue_image / analyze_face_image，再综合成结构化结论。
"""
from __future__ import annotations

from app.config import settings
from app.protocol.llm import get_provider, image_content
from app.skills.types import SkillManifest, ToolSpec

VISION_SYSTEM = (
    "你是中医望诊影像分析助手。请基于图像给出客观、简洁的望诊观察"
    "（舌体/舌苔/舌态/面色/神/患处），仅描述可见事实，不做诊断结论，使用中文。"
)


async def _analyze(path: str, kind: str) -> dict:
    provider = get_provider()
    if not provider:
        return {"ok": False, "error": "未配置视觉模型(llm_server)，无法分析图像"}
    try:
        content = await provider.chat(
            messages=[
                {"role": "system", "content": VISION_SYSTEM},
                {"role": "user", "content": [
                    {"type": "text", "text": f"请描述这张{kind}图像中的望诊特征。"},
                    image_content(path),
                ]},
            ],
            model=settings.resolve_model("vision-default"),
            json_mode=False,
        )
    except Exception as e:  # noqa: BLE001  视觉模型不可用/图片损坏 -> 交由 rule 兜底
        return {"ok": False, "error": f"视觉分析失败：{e}"}
    return {"ok": True, "kind": kind, "analysis": str(content or "").strip()}


async def analyze_tongue_image(path: str) -> dict:
    """分析舌象图片，返回舌体/舌苔/舌态观察。"""
    return await _analyze(path, "舌象")


async def analyze_face_image(path: str) -> dict:
    """分析面象/神色图片，返回面色/神色观察。"""
    return await _analyze(path, "面象")


async def analyze_general_image(path: str, region: str = "患处") -> dict:
    """分析任意部位的中医望诊图像（如皮肤、患处、局部体征、小儿指纹等）。

    region 用于提示模型关注部位，不影响调用方式；返回该部位的客观望诊观察。
    """
    return await _analyze(path, region)


SKILL = SkillManifest(
    name="tcm-vision",
    version="0.1.0",
    description="中医望诊多模态技能：借助独立部署的 Qwen3-VL 视觉模型（原生多模态）分析舌象/面象图像。供望诊 LLM 调用。",
    tools=[
        ToolSpec(
            name="analyze_tongue_image",
            description="分析舌象图片，返回舌体/舌苔/舌态的客观观察文字。",
            parameters={
                "type": "object",
                "properties": {"path": {"type": "string", "description": "图像在本服务的本地路径"}},
                "required": ["path"],
            },
            capability="diagnosis.inspection",
        ),
        ToolSpec(
            name="analyze_face_image",
            description="分析面象/神色图片，返回面色/神色的客观观察文字。",
            parameters={
                "type": "object",
                "properties": {"path": {"type": "string", "description": "图像在本服务的本地路径"}},
                "required": ["path"],
            },
            capability="diagnosis.inspection",
        ),
        ToolSpec(
            name="analyze_general_image",
            description="分析任意部位的中医望诊图像（皮肤、患处、局部体征、小儿指纹等），"
                        "返回该部位的客观望诊观察文字。region 用于提示关注部位。",
            parameters={
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "图像在本服务的本地路径"},
                    "region": {"type": "string", "description": "关注部位/区域，如 患处/皮肤/手部",
                                "default": "患处"},
                },
                "required": ["path"],
            },
            capability="diagnosis.inspection",
        ),
    ],
)

HANDLERS = {
    "analyze_tongue_image": analyze_tongue_image,
    "analyze_face_image": analyze_face_image,
    "analyze_general_image": analyze_general_image,
}
