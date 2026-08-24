"""SKILL 装载器：从文件系统发现并加载技能模块。

技能可以是：
- 一个包目录（含 ``__init__.py``），或
- 一个独立的 ``.py`` 文件。

每个技能模块必须定义：
- ``SKILL``：``SkillManifest`` 或等价的 dict；
- ``HANDLERS``：``{工具名: 可调用对象}``，键需覆盖 ``SKILL.tools`` 中的所有 ``name``。

装载方式：
- ``discover_skills(directory)``：启动自动发现；
- ``load_skill_by_name(name, skills_dir)`` / ``load_skill_from_path(path)``：运行时热装载。
"""
from __future__ import annotations

import importlib.util
import logging
from pathlib import Path
from types import ModuleType

from .registry import skill_registry
from .types import SkillError, SkillManifest

logger = logging.getLogger("skills")


def _import_from_path(path: Path, modname: str) -> ModuleType:
    spec = importlib.util.spec_from_file_location(modname, str(path))
    if spec is None or spec.loader is None:
        raise SkillError(f"无法从 {path} 导入技能模块")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)  # type: ignore[union-attr]
    return mod


def load_skill_from_module(mod: ModuleType, source: str = "") -> SkillManifest:
    manifest = getattr(mod, "SKILL", None)
    if manifest is None:
        raise SkillError("技能模块必须定义 SKILL 清单")
    handlers = getattr(mod, "HANDLERS", None) or {}
    return skill_registry.register_skill(manifest, handlers, source)


def load_skill_from_path(path: str | Path) -> SkillManifest:
    p = Path(path)
    if p.is_dir():
        p = p / "__init__.py"
    if not p.exists():
        raise SkillError(f"技能路径不存在：{path}")
    mod = _import_from_path(p, f"tcm_skill_{p.stem}")
    return load_skill_from_module(mod, source=str(p))


def load_skill_by_name(name: str, skills_dir: str | Path) -> SkillManifest:
    base = Path(skills_dir)
    d = base / name
    if d.is_dir():
        return load_skill_from_path(d)
    f = base / f"{name}.py"
    if f.exists():
        return load_skill_from_path(f)
    raise SkillError(f"技能 '{name}' 未在 {skills_dir} 中找到")


def discover_skills(directory: str | Path) -> list[SkillManifest]:
    """扫描目录，加载其中所有技能包/模块（启动发现用）。

    仅当文件/包内确实定义了 ``SKILL`` 时才真正导入，避免把框架模块
    （types/registry/loader/toolcall 等）误当作技能去导入而触发相对导入报错。
    """
    base = Path(directory)
    if not base.exists():
        logger.warning("技能目录不存在，跳过发现：%s", base)
        return []
    loaded: list[SkillManifest] = []
    for entry in sorted(base.iterdir()):
        try:
            if entry.is_dir() and (entry / "__init__.py").exists():
                if "SKILL" not in (entry / "__init__.py").read_text(
                    encoding="utf-8", errors="ignore"
                ):
                    continue
                mod = _import_from_path(entry / "__init__.py", f"tcm_skill_{entry.name}")
                loaded.append(load_skill_from_module(mod, source=str(entry)))
            elif entry.is_file() and entry.suffix == ".py" and entry.name != "__init__.py":
                if "SKILL" not in entry.read_text(encoding="utf-8", errors="ignore"):
                    continue
                mod = _import_from_path(entry, f"tcm_skill_{entry.stem}")
                loaded.append(load_skill_from_module(mod, source=str(entry)))
        except (SkillError, ImportError) as e:  # 单个技能失败不应阻断整体启动
            logger.warning("跳过技能 %s：%s", entry, e)
    return loaded
