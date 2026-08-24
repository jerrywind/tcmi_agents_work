"""SKILL 全局注册表：管理技能与工具的注册、查询、按 capability 过滤与执行。

进程内单例 ``skill_registry`` 在应用启动时由 ``discover_skills`` 填充，
运行时也可通过 API 热装载/卸载。所有查询均线程/协程安全（注册只发生在
导入期与受控的 API 调用中）。
"""
from __future__ import annotations

import asyncio
from typing import Any, Callable, Awaitable

from .types import SkillError, SkillManifest, ToolSpec

Handler = Callable[..., Any] | Callable[..., Awaitable[Any]]


class _ToolEntry:
    def __init__(self, spec: ToolSpec, handler: Handler, skill_name: str) -> None:
        self.spec = spec
        self.handler = handler
        self.skill_name = skill_name


class _SkillEntry:
    def __init__(self, manifest: SkillManifest, handlers: dict[str, Handler], source: str = "") -> None:
        self.manifest = manifest
        self.handlers = handlers
        self.source = source


class SkillRegistry:
    def __init__(self) -> None:
        self._skills: dict[str, _SkillEntry] = {}
        self._tools: dict[str, _ToolEntry] = {}

    # ---- 注册 / 卸载 ----
    def register_skill(self, manifest: SkillManifest | dict, handlers: dict[str, Handler],
                       source: str = "") -> SkillManifest:
        if isinstance(manifest, dict):
            manifest = SkillManifest(**manifest)
        # 同名热装载：先卸载旧版，避免工具残留
        if manifest.name in self._skills:
            self.unload(manifest.name)
        for t in manifest.tools:
            if t.name not in handlers:
                raise SkillError(f"skill '{manifest.name}' 工具 '{t.name}' 缺少 handler")
        self._skills[manifest.name] = _SkillEntry(manifest, handlers, source)
        for t in manifest.tools:
            self._tools[t.name] = _ToolEntry(t, handlers[t.name], manifest.name)
        return manifest

    def unload(self, name: str) -> bool:
        entry = self._skills.pop(name, None)
        if entry is None:
            return False
        for t in entry.manifest.tools:
            self._tools.pop(t.name, None)
        return True

    # ---- 查询 ----
    def get_skill(self, name: str) -> _SkillEntry | None:
        return self._skills.get(name)

    def list_skills(self) -> list[SkillManifest]:
        return [e.manifest for e in self._skills.values()]

    def list_tools(self) -> list[ToolSpec]:
        return [t.spec for t in self._tools.values()]

    def tools_for(self, capability: str) -> list[dict]:
        """返回某 capability 下、可供 LLM function-calling 的工具 schema 列表。

        工具的 ``capability`` 可以是：
        - ``""``：对所有 capability 开放；
        - 单个字符串：仅对该 capability 开放；
        - 字符串列表：对列表中任一 capability 开放。
        """
        out: list[dict] = []
        for t in self._tools.values():
            if _capability_matches(t.spec.capability, capability):
                out.append({
                    "type": "function",
                    "function": {
                        "name": t.spec.name,
                        "description": t.spec.description,
                        "parameters": t.spec.parameters,
                    },
                })
        return out

    # ---- 执行 ----
    async def run_tool(self, name: str, args: dict | None) -> Any:
        t = self._tools.get(name)
        if t is None:
            raise SkillError(f"未知工具：{name}")
        if asyncio.iscoroutinefunction(t.handler):
            return await t.handler(**(args or {}))
        return t.handler(**(args or {}))


def _capability_matches(spec_cap: str | list[str], req_cap: str) -> bool:
    if spec_cap == "":
        return True
    if isinstance(spec_cap, list):
        return req_cap in spec_cap
    return spec_cap == req_cap


# 进程内单例
skill_registry = SkillRegistry()
