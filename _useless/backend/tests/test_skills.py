"""SKILL 装载/注册/执行测试。

覆盖：模块装载、按能力过滤工具、同步/异步工具执行、卸载、目录发现，
以及清单缺失/工具缺 handler 的可预期错误。
"""
import textwrap

import pytest

from app.skills.loader import (
    discover_skills,
    load_skill_by_name,
    load_skill_from_path,
)
from app.skills.registry import skill_registry
from app.skills.types import SkillError, SkillManifest, ToolSpec

SKILL_MODULE = textwrap.dedent(
    """
    from app.skills.types import SkillManifest, ToolSpec

    SKILL = SkillManifest(
        name="demo",
        version="0.1.0",
        description="demo skill",
        tools=[
            ToolSpec(name="add", description="add two ints",
                     parameters={"type": "object",
                                 "properties": {"a": {"type": "integer"},
                                                "b": {"type": "integer"}},
                                 "required": ["a", "b"]},
                     capability="treatment.plan"),
            ToolSpec(name="noop", description="always ok",
                     parameters={}, capability=""),
        ],
    )

    def add(a, b):
        return {"sum": a + b}

    def noop():
        return {"ok": True}

    HANDLERS = {"add": add, "noop": noop}
    """
)


@pytest.fixture
def demo_skill(tmp_path):
    f = tmp_path / "demo.py"
    f.write_text(SKILL_MODULE)
    manifest = load_skill_from_path(f)
    yield manifest
    skill_registry.unload("demo")


def test_load_and_query(demo_skill):
    assert demo_skill.name == "demo"
    names = {t.name for t in skill_registry.list_tools()}
    # 全局注册表在启动时已装载内置 tcm-kb 技能，故断言子集而非全等
    assert {"add", "noop"} <= names

    # capability 过滤：add 仅对 treatment.plan 开放；noop("") 对所有能力开放
    tp = skill_registry.tools_for("treatment.plan")
    tp_names = {t["function"]["name"] for t in tp}
    assert {"add", "noop"} <= tp_names
    safety = skill_registry.tools_for("diagnosis.safety")
    safety_names = {t["function"]["name"] for t in safety}
    assert "add" not in safety_names
    assert "noop" in safety_names


async def test_builtin_skill_handlers():
    # 内置 tcm-kb 在启动时已装载，直接调用其工具验证实现
    r1 = await skill_registry.run_tool("lookup_herb", {"herb": "黄连"})
    assert r1["found"] is True and "功效" in r1
    r2 = await skill_registry.run_tool("lookup_syndrome_treatment", {"syndrome": "脾胃湿热"})
    assert r2["found"] is True and len(r2["treatments"]) > 0
    r3 = await skill_registry.run_tool("lookup_herb", {"herb": "未知药材"})
    assert r3["found"] is False


async def test_run_tool_sync(demo_skill):
    res = await skill_registry.run_tool("add", {"a": 2, "b": 3})
    assert res == {"sum": 5}
    res2 = await skill_registry.run_tool("noop", {})
    assert res2 == {"ok": True}


async def test_unknown_tool_raises(demo_skill):
    with pytest.raises(SkillError):
        await skill_registry.run_tool("missing", {})


def test_unload_removes_tools(demo_skill):
    assert skill_registry.unload("demo") is True
    remaining = {t.name for t in skill_registry.list_tools()}
    assert "add" not in remaining and "noop" not in remaining
    assert skill_registry.unload("demo") is False  # 重复卸载返回 False


def test_missing_manifest_raises(tmp_path):
    f = tmp_path / "bad.py"
    f.write_text("X = 1\n")
    with pytest.raises(SkillError):
        load_skill_from_path(f)


def test_missing_handler_raises(tmp_path):
    f = tmp_path / "bad2.py"
    f.write_text(
        "from app.skills.types import SkillManifest, ToolSpec\n"
        "SKILL = SkillManifest(name='b', tools=[ToolSpec(name='t', description='d')])\n"
        "HANDLERS = {}\n"
    )
    with pytest.raises(SkillError):
        load_skill_from_path(f)


def test_discover_skills(tmp_path):
    pkg = tmp_path / "sk1"
    pkg.mkdir()
    (pkg / "__init__.py").write_text(SKILL_MODULE)
    loaded = discover_skills(tmp_path)
    names = {m.name for m in loaded}
    assert "demo" in names
    # 清理全局注册表
    skill_registry.unload("demo")


def test_load_by_name(tmp_path, monkeypatch):
    # 把临时目录当成 skills_dir，按名称装载
    pkg = tmp_path / "demo_pkg"
    pkg.mkdir()
    (pkg / "__init__.py").write_text(SKILL_MODULE)
    m = load_skill_by_name("demo_pkg", tmp_path)
    assert m.name == "demo"
    skill_registry.unload("demo")


def test_load_by_name_not_found(tmp_path):
    with pytest.raises(SkillError):
        load_skill_by_name("nope", tmp_path)
