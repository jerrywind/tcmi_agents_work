"""后端测试公共 fixtures：构造会话、协议请求与内存存储。"""
from __future__ import annotations

import sys
from pathlib import Path

# 确保 backend 根目录在 sys.path，便于直接 import app 包
ROOT = Path(__file__).resolve().parent.parent
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import copy
import os

import pytest

import app.agents  # noqa: F401  触发所有 sub-agent 注册
from app.config import settings as _config_settings
from app.models.schemas import Consultation, Evidence, Patient
from app.protocol import llm as llm_mod
from app.protocol.base import Capability
from app.protocol.registry import build_request
from app.skills.registry import skill_registry

# 需要在每个测试间隔离的 API Key 环境变量（避免某测试 setenv 后泄漏到
# 依赖「无 Key 回退 Mock」的用例）。这些键由 Settings.llm_api_key 等 property 读取。
_API_KEY_ENV = (
    "TCM_LLM_API_KEY",
    "TCM_LLM_BASE_URL",
    "TCM_LLM_VISION_BASE_URL",
    "TCM_LLM_VISION_MODEL",
    "TCM_LLM_TEXT_MODEL",
    "TCM_LLM_PROVIDER",
    "TCM_ROUTING_FILE",
)


@pytest.fixture(autouse=True)
def _isolate_config_and_provider():
    """每个测试间隔离全局配置与 provider，消除跨测试污染。

    部分测试会原地修改 ``config.settings.routing`` / ``config.settings.llm``，或直接
    ``setenv`` API Key 相关环境变量。若还原不充分，会泄漏到后续依赖「无 Key 回退
    Mock」「默认 routing」的用例。这里在 setup 时记录快照，teardown 时整体还原，
    并清空 LLM provider 缓存。
    """
    routing_snap = copy.deepcopy(_config_settings.routing)
    llm_snap = copy.deepcopy(_config_settings.llm)
    yield
    _config_settings.routing = routing_snap
    _config_settings.llm = llm_snap
    # 强制清除 API Key 相关环境变量，避免任何测试（含直接使用 os.environ 赋值、
    # 或 monkeypatch 还原不充分的情况）泄漏 TCM_LLM_API_KEY 等到后续依赖「无 Key
    # 回退 Mock」的用例。需要 Key 的测试会自行 setenv，teardown 时由各自的
    # monkeypatch 还原，最终仍被此处清掉。
    for k in _API_KEY_ENV:
        os.environ.pop(k, None)
    llm_mod._provider = None


@pytest.fixture(autouse=True)
def _isolate_skill_registry():
    """每个测试间隔离全局 skill 注册表，消除跨测试的工具残留污染。

    ``skill_registry`` 是进程内单例，由 ``app.main`` 导入时 ``discover_skills`` 填充。
    某些测试会重建 app（如删除 ``app.*`` 模块缓存后重新 import ``app.main``），触发
    ``discover_skills`` 再次装载；若此时全局注册表未正确清理，残留的、capability 匹配
    某能力的工具（如闻诊的 ``lookup_voice_pattern``）会让后续``run_tool_loop`` 误入
    function-calling 分支，导致依赖「无工具 -> 单次 json_mode」的 LLM 测试拿到空结果。
    这里在 setup 时深拷贝注册表快照，teardown 时整体还原。
    """
    skills_snap = dict(skill_registry._skills)
    tools_snap = dict(skill_registry._tools)
    yield
    skill_registry._skills = dict(skills_snap)
    skill_registry._tools = dict(tools_snap)


@pytest.fixture
def make_consultation():
    """构造一个问诊会话，便于驱动编排器。"""

    def _make(complaint: str = "我最近身体不太舒服", gender: str = "未知",
              self_report: dict | None = None, status: str = "created") -> Consultation:
        c = Consultation(patient=Patient(gender=gender), complaint=complaint,
                         self_report=self_report or {})
        if status != "created":
            c.status = status
        return c

    return _make


@pytest.fixture
def build_req():
    """按统一协议构造 AgentRequest（自动带上 routing 的 model/options）。"""

    def _b(cap: Capability, payload: dict | None = None,
           evidences: list | None = None, hypotheses: list | None = None,
           asked_keys: list | None = None) -> "AgentRequest":  # noqa: F821
        return build_request(
            cap, session_id="test-session", round=0,
            payload=payload or {}, evidences=evidences or [],
            hypotheses=hypotheses or [], asked_keys=asked_keys or [],
        )

    return _b


@pytest.fixture
def sample_evidences():
    """脾胃湿热典型证据，用于触发收敛。"""

    def _make() -> list[Evidence]:
        data = [
            ("thirst", "口苦"), ("smell", "口臭"), ("stool", "粘滞不爽"),
            ("head_body", "肢体困重"), ("appetite", "食欲不振"),
        ]
        return [Evidence(key=k, value=v, source="闻", confidence=1.0) for k, v in data]

    return _make
