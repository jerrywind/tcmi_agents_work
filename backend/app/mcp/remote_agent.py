"""远程 Sub-Agent 桥：把某个 capability 路由到外部 MCP Server。

`SubAgent` 协议本身是**无状态 + JSON 信封**的，因此天然可远程化。本模块为
7 个 capability 各注册一个 ``impl_name = "mcp"`` 的实现，编排器无需任何改动。

启用方式（`routing.yaml`）::

    routing:
      diagnosis.inspection:
        impl: mcp
        options:
          server: vision_farm      # mcp.clients 中的连接名
          tool: agent_inspection   # 可选，默认按 capability 推断

容错：远端未连接/超时/报错时返回 ``status=error`` 信封，
编排器沿用既有降级路径，不会中断问诊。
"""
from __future__ import annotations

import logging
from typing import Any, ClassVar

from ..protocol.base import AgentRequest, AgentResponse, Capability, SubAgent
from ..protocol.registry import register
from .tools.agents import CAPABILITY_TOOL

logger = logging.getLogger(__name__)


def _get_hub():
    """延迟获取 Hub 单例，便于测试替换。"""
    from .client import tool_hub
    return tool_hub


class McpRemoteAgent(SubAgent):
    """通用远程 Sub-Agent：经 MCP 调用远端同能力实现。"""

    impl_name: ClassVar[str] = "mcp"
    description: ClassVar[str] = "经 MCP 协议调用远程 Sub-Agent 实现"

    def _resolve_target(self, req: AgentRequest) -> tuple[str, str]:
        """从 options 解析远端 server 与 tool 名。"""
        opts = req.options or {}
        server = opts.get("server") or opts.get("server_name") or ""
        if not server:
            raise ValueError(
                f"capability '{self.capability.value}' 配置 impl=mcp 但未指定 "
                f"options.server（应为 mcp.clients 中的连接名）"
            )
        tool = opts.get("tool") or CAPABILITY_TOOL.get(self.capability.value, "run_agent")
        return server, tool

    def _build_args(self, req: AgentRequest, tool: str) -> dict[str, Any]:
        """把请求信封序列化为 MCP 工具参数。"""
        args: dict[str, Any] = {
            "session_id": req.session_id,
            "round": req.round,
            "payload": req.payload,
            "evidences": [e.model_dump(mode="json") for e in req.evidences],
            "hypotheses": [h.model_dump(mode="json") for h in req.hypotheses],
            "asked_keys": list(req.asked_keys),
            "model": req.model,
        }
        # 同时铺平 payload，兼容按扁平字段声明 schema 的远端实现
        if isinstance(req.payload, dict):
            for k, v in req.payload.items():
                args.setdefault(k, v)
        if tool == "run_agent":
            args["capability"] = self.capability.value
        return args

    def _parse_response(self, raw: Any) -> AgentResponse:
        """把远端返回反序列化为 AgentResponse 信封。"""
        if not isinstance(raw, dict):
            raise ValueError(f"远端返回格式非法：{type(raw).__name__}")
        if raw.get("error") and "capability" not in raw:
            # 远端以 {"error": ...} 形式报错
            raise ValueError(str(raw["error"]))
        data = dict(raw)
        data["capability"] = self.capability.value  # 以本地 capability 为准
        data.pop("request_id", None)                 # 由 run() 统一回填
        try:
            return AgentResponse(**data)
        except Exception as exc:  # noqa: BLE001 字段不兼容时给出清晰错误
            raise ValueError(f"远端响应无法解析为 AgentResponse: {exc}") from exc

    async def handle(self, req: AgentRequest) -> AgentResponse:
        server, tool = self._resolve_target(req)
        hub = _get_hub()
        if not hub.is_connected(server):
            raise RuntimeError(f"MCP server '{server}' 未连接，无法执行远程 {self.capability.value}")
        raw = await hub.call(server, tool, self._build_args(req, tool))
        resp = self._parse_response(raw)
        resp.meta = {**(resp.meta or {}), "remote_server": server, "remote_tool": tool}
        return resp


# ---------------------------------------------------------------------------
# 为每个 capability 注册一个 impl="mcp" 的子类
# ---------------------------------------------------------------------------
def _make(cap: Capability, cls_name: str) -> type[SubAgent]:
    cls = type(cls_name, (McpRemoteAgent,), {
        "capability": cap,
        "description": f"经 MCP 调用远程「{cap.value}」实现",
    })
    return register(cls)


McpInspectionAgent = _make(Capability.INSPECTION, "McpInspectionAgent")
McpListeningAgent = _make(Capability.LISTENING, "McpListeningAgent")
McpInquiryAgent = _make(Capability.INQUIRY, "McpInquiryAgent")
McpPalpationAgent = _make(Capability.PALPATION, "McpPalpationAgent")
McpDifferentiationAgent = _make(Capability.DIFFERENTIATION, "McpDifferentiationAgent")
McpTreatmentAgent = _make(Capability.TREATMENT, "McpTreatmentAgent")
McpSafetyAgent = _make(Capability.SAFETY, "McpSafetyAgent")

__all__ = [
    "McpRemoteAgent", "McpInspectionAgent", "McpListeningAgent", "McpInquiryAgent",
    "McpPalpationAgent", "McpDifferentiationAgent", "McpTreatmentAgent", "McpSafetyAgent",
]
