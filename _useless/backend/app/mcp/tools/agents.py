"""Agent 级 MCP 工具：把每个 Sub-Agent capability 暴露为一个独立的 MCP 工具。

与会话级工具的区别：
- **无状态**：不创建/依赖 Consultation，输入信封即完整上下文，可并发调用；
- **原子化**：外部调用方可以只借用"望诊"或"辨证"单项能力，而不必跑完整流程；
- **同构**：直接复用 ``AgentRequest`` / ``AgentResponse`` 协议信封，
  因此本模块同时也是"远程 Sub-Agent"的服务端实现（对端见 ``mcp/remote_agent.py``）。

工具清单：
    agent_inspection / agent_listening / agent_inquiry / agent_palpation /
    agent_differentiation / agent_treatment / agent_safety
    + run_agent（通用信封入口） + list_agent_capabilities（自省）
"""
from __future__ import annotations

from typing import Any

from mcp.types import Tool

from ...protocol.base import AgentRequest, AgentResponse, Capability
from ...protocol.registry import available_impls, build_request, resolve

# capability -> (工具名, 中文名, 输入说明, payload 属性)
_EVIDENCE_SCHEMA = {
    "type": "array",
    "description": "证据池快照（只读），元素为 Evidence 对象",
    "items": {"type": "object"},
}
_HYPOTHESIS_SCHEMA = {
    "type": "array",
    "description": "当前候选证候快照（只读），元素为 Hypothesis 对象",
    "items": {"type": "object"},
}


def _envelope_props(extra: dict[str, Any] | None = None) -> dict[str, Any]:
    """构造统一信封字段 + capability 特有 payload 字段。"""
    props: dict[str, Any] = {
        "session_id": {"type": "string", "description": "调用方自定义会话标识（可选）"},
        "round": {"type": "integer", "description": "当前轮次，默认 0"},
        "evidences": _EVIDENCE_SCHEMA,
        "hypotheses": _HYPOTHESIS_SCHEMA,
        "asked_keys": {"type": "array", "items": {"type": "string"},
                       "description": "已提问过的特征键，用于避免重复提问"},
        "model": {"type": "string", "description": "覆盖路由指定的逻辑模型名（可选）"},
    }
    if extra:
        props.update(extra)
    return props


_AGENT_TOOLS: dict[str, dict[str, Any]] = {
    "agent_inspection": {
        "capability": Capability.INSPECTION,
        "description": "望诊：分析舌象/面相/患处/掌纹图像与自述，产出结构化证据（evidences）。",
        "payload_keys": ["images", "self_report"],
        "extra": {
            "images": {
                "type": "array",
                "description": "图像列表，元素形如 {type: tongue|face|lesion|palm_left|palm_right, path: 本地路径, url: 可选}",
                "items": {"type": "object"},
            },
            "self_report": {"type": "object", "description": "患者自述的望诊相关信息（可选）"},
        },
    },
    "agent_listening": {
        "capability": Capability.LISTENING,
        "description": "闻诊：从主诉文本中提取声音/气味相关线索，产出结构化证据（evidences）。",
        "payload_keys": ["text"],
        "extra": {"text": {"type": "string", "description": "主诉或描述文本"}},
    },
    "agent_inquiry": {
        "capability": Capability.INQUIRY,
        "description": "问诊：依据当前证据池与候选证候，计算信息增益最高的下一个问题（question）。",
        "payload_keys": ["gender", "age", "qa"],
        "extra": {
            "gender": {"type": "string", "description": "患者性别，影响可问项（如经带胎产）"},
            "age": {"type": "integer", "description": "患者年龄"},
        },
    },
    "agent_palpation": {
        "capability": Capability.PALPATION,
        "description": "切诊：解析 PPG 脉搏信号或自述脉感，产出脉率/脉力/脉形等证据（evidences）。",
        "payload_keys": ["ppg", "self_report", "text"],
        "extra": {
            "ppg": {"type": "object",
                    "description": "PPG 解析结果，形如 {rate_bpm, strength, shape, ...}"},
            "self_report": {"type": "object", "description": "自述脉感（可选）"},
            "text": {"type": "string", "description": "描述文本（可选）"},
        },
    },
    "agent_differentiation": {
        "capability": Capability.DIFFERENTIATION,
        "description": "辨证：根据证据池推断候选证候及其置信度，产出 hypotheses（按 confidence 降序）。",
        "payload_keys": [],
        "extra": {},
    },
    "agent_treatment": {
        "capability": Capability.TREATMENT,
        "description": "诊疗方案：依据确定的证候产出治法方药/针灸/西医检查建议（plans），必要时追问（question）。",
        "payload_keys": ["diagnoses", "qa", "patient"],
        "extra": {
            "diagnoses": {"type": "array", "items": {"type": "object"},
                          "description": "已确定的证候列表"},
            "qa": {"type": "array", "items": {"type": "object"},
                   "description": "已完成的个性化追问问答对"},
            "patient": {"type": "object", "description": "患者基本信息 {age, gender, region}"},
        },
    },
    "agent_safety": {
        "capability": Capability.SAFETY,
        "description": "安全审查：识别文本中的红旗症状/危险指征，产出 alerts（danger|warning）。",
        "payload_keys": ["text"],
        "extra": {"text": {"type": "string", "description": "待审查文本"}},
    },
}

# 工具名 -> capability 的快速索引
TOOL_CAPABILITY: dict[str, Capability] = {
    name: meta["capability"] for name, meta in _AGENT_TOOLS.items()
}
# capability 值 -> 工具名（供 remote_agent 推断默认工具名）
CAPABILITY_TOOL: dict[str, str] = {
    meta["capability"].value: name for name, meta in _AGENT_TOOLS.items()
}


def list_tools() -> list[Tool]:
    """返回 Agent 级 MCP 工具定义。"""
    tools: list[Tool] = []
    for name, meta in _AGENT_TOOLS.items():
        tools.append(Tool(
            name=name,
            description=meta["description"],
            inputSchema={
                "type": "object",
                "properties": _envelope_props(meta.get("extra")),
            },
        ))
    tools.append(Tool(
        name="run_agent",
        description="通用 Sub-Agent 入口：直接投递协议信封。capability 取 "
                    "list_agent_capabilities 返回的值，payload 为该能力特有输入。",
        inputSchema={
            "type": "object",
            "properties": {
                "capability": {
                    "type": "string",
                    "enum": [c.value for c in Capability],
                    "description": "目标能力",
                },
                "payload": {"type": "object", "description": "能力特有输入"},
                **_envelope_props(),
            },
            "required": ["capability"],
        },
    ))
    tools.append(Tool(
        name="list_agent_capabilities",
        description="列出全部中医 Sub-Agent 能力、当前生效实现与可选实现，用于自省与调试。",
        inputSchema={"type": "object", "properties": {}},
    ))
    return tools


def capabilities_overview() -> list[dict]:
    """能力总览：capability / 工具名 / 当前 impl / 可用 impl 列表。"""
    out: list[dict] = []
    for cap in Capability:
        try:
            _, route = resolve(cap)
            active = route.get("impl", "")
            requested = route.get("requested_impl", active)
        except KeyError:
            active, requested = "", ""
        out.append({
            "capability": cap.value,
            "tool": CAPABILITY_TOOL.get(cap.value, ""),
            "impl": active,
            "requested_impl": requested,
            "degraded": bool(requested) and requested != active,
            "available_impls": available_impls(cap),
            "description": _AGENT_TOOLS.get(CAPABILITY_TOOL.get(cap.value, ""), {}).get("description", ""),
        })
    return out


def _build_payload(args: dict, payload_keys: list[str]) -> dict:
    """从扁平参数中抽取该 capability 的 payload 字段。

    同时兼容调用方直接传 ``payload`` 对象的写法（两者合并，扁平字段优先）。
    """
    payload: dict[str, Any] = dict(args.get("payload") or {})
    for k in payload_keys:
        if args.get(k) is not None:
            payload[k] = args[k]
    return payload


async def dispatch(capability: Capability, args: dict, payload_keys: list[str] | None = None) -> dict:
    """执行一次 Sub-Agent 调用并返回可 JSON 序列化的响应信封。"""
    agent, _route = resolve(capability)
    payload = _build_payload(args, payload_keys or [])
    req: AgentRequest = build_request(
        capability,
        model=args.get("model") or None,
        session_id=str(args.get("session_id") or ""),
        round=int(args.get("round") or 0),
        payload=payload,
        evidences=args.get("evidences") or [],
        hypotheses=args.get("hypotheses") or [],
        asked_keys=args.get("asked_keys") or [],
    )
    resp: AgentResponse = await agent.run(req)
    return resp.model_dump(mode="json")


async def handle_call(name: str, args: dict) -> dict | list | None:
    """处理 Agent 级工具调用；工具名不属于本模块时返回 None。"""
    if name == "list_agent_capabilities":
        return capabilities_overview()

    if name == "run_agent":
        raw = args.get("capability") or ""
        try:
            cap = Capability(raw)
        except ValueError:
            raise ValueError(
                f"未知 capability: {raw}，可选值：{[c.value for c in Capability]}"
            ) from None
        tool_name = CAPABILITY_TOOL.get(cap.value, "")
        keys = _AGENT_TOOLS.get(tool_name, {}).get("payload_keys", [])
        return await dispatch(cap, args, keys)

    meta = _AGENT_TOOLS.get(name)
    if meta is None:
        return None
    return await dispatch(meta["capability"], args, meta.get("payload_keys", []))
