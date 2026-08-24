"""Sub-Agent 注册表：capability + impl_name -> 实现类。

用法：
    @register
    class TongueRuleAgent(SubAgent):
        capability = Capability.INSPECTION
        impl_name = "rule"

切换实现只需修改 routing.yaml 中对应 capability 的 impl 字段。
"""
from __future__ import annotations

from typing import Any

from ..config import settings
from .base import AgentRequest, Capability, SubAgent

_REGISTRY: dict[tuple[str, str], type[SubAgent]] = {}
_INSTANCES: dict[tuple[str, str], SubAgent] = {}


def register(cls: type[SubAgent]) -> type[SubAgent]:
    key = (cls.capability.value, cls.impl_name)
    if key in _REGISTRY:
        raise ValueError(f"duplicated sub-agent: {key}")
    _REGISTRY[key] = cls
    return cls


def available_impls(capability: Capability) -> list[str]:
    return [impl for (cap, impl) in _REGISTRY if cap == capability.value]


def resolve(capability: Capability) -> tuple[SubAgent, dict[str, Any]]:
    """按路由配置解析实现实例；实现缺失时降级到 rule。

    返回的 route 保留两字段用于可观测：``requested_impl``（routing.yaml 期望的实现）、
    ``impl``（实际生效的实现，实现未注册时回退为 ``rule``）。两者不一致即表示发生了降级。
    """
    route = settings.route_of(capability.value)
    requested_impl = route["impl"]
    impl = requested_impl
    key = (capability.value, impl)
    if key not in _REGISTRY:
        impl = "rule"  # 兜底
        key = (capability.value, impl)
        if key not in _REGISTRY:
            raise KeyError(f"no implementation for {capability.value}")
    if impl != requested_impl:
        route = dict(route)
        route["impl"] = impl
    route["requested_impl"] = requested_impl
    if key not in _INSTANCES:
        _INSTANCES[key] = _REGISTRY[key]()
    return _INSTANCES[key], route


def build_request(capability: Capability, model: str | None = None, **kwargs) -> AgentRequest:
    """构造带路由参数的请求信封。

    ``model`` 为可选覆盖值：不传时取路由配置中的 model；传入时（如自适应路由
    切轻模型）覆盖。避免与 kwargs 同时传 model 造成重复关键字参数。
    """
    route = settings.route_of(capability.value)
    resolved_model: str = model if model is not None else route.get("model", "")
    return AgentRequest(
        capability=capability,
        model=resolved_model,
        options=route.get("options", {}),
        **kwargs,
    )
