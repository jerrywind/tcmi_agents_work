"""SKILL 类型定义：工具规格与技能清单。

SKILL = 一组 LLM 可调用工具（function calling）。每个技能是一个独立包/模块，
通过声明 ``SKILL`` 清单 + ``HANDLERS`` 映射把自己的工具注册进全局注册表，
编排器/LLM agent 即可在推理时按需调用。
"""
from __future__ import annotations

from pydantic import BaseModel, Field


class SkillError(Exception):
    """技能加载/调用过程中的可预期错误（如清单缺失、工具未注册）。"""


class ToolSpec(BaseModel):
    """单个工具的声明（OpenAI function-calling 兼容）。"""

    name: str
    description: str
    parameters: dict = Field(default_factory=dict)  # JSON Schema
    # 该工具可被哪些 capability 的 LLM 调用；"" 表示对所有 capability 开放。
    # 也支持 list[str]（如 ["diagnosis.differentiation", "treatment.plan"]）
    # 表示对其中任一 capability 开放。
    capability: str | list[str] = ""


class SkillManifest(BaseModel):
    """技能清单：名称、版本、描述与所含工具。"""

    name: str
    version: str = "0.1.0"
    description: str = ""
    tools: list[ToolSpec] = Field(default_factory=list)
