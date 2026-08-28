"""SKILL 子系统入口。

提供全局注册表 ``skill_registry``、类型 ``SkillManifest/ToolSpec/SkillError``，
以及装载函数 ``discover_skills / load_skill_by_name / load_skill_from_path``。
"""
from __future__ import annotations

from .loader import (
    discover_skills,
    load_skill_by_name,
    load_skill_from_module,
    load_skill_from_path,
)
from .registry import SkillRegistry, skill_registry
from .toolcall import run_tool_loop
from .types import SkillError, SkillManifest, ToolSpec

__all__ = [
    "SkillManifest",
    "ToolSpec",
    "SkillError",
    "SkillRegistry",
    "skill_registry",
    "run_tool_loop",
    "discover_skills",
    "load_skill_by_name",
    "load_skill_from_module",
    "load_skill_from_path",
]
