"""OpenAI 兼容网关 + Agent 接口。

对外保持与 backend `OpenAICompatProvider` 完全兼容：
  - POST /v1/chat/completions    （透传 LM Studio，带 prompt 优化；x-tcm-agent 头可启用网关内 agent）
  - POST /v1/responses           （透传 LM Studio Responses API）
  - POST /v1/embeddings          （透传 LM Studio，供 RAG 复用）
  - GET  /v1/models              （拉取 LM Studio 模型列表）
新增 Agent 能力：
  - POST /v1/agent/run           （prompt 优化 + tool calling + MCP + agent 循环）
  - GET  /v1/agent/tools         （查看当前可用工具）
健康检查：
  - GET  /healthz                （含上游 LM Studio 连通性）
"""
from __future__ import annotations

import logging
import time
import uuid
from typing import Any

from fastapi import APIRouter, HTTPException, Request
from fastapi.responses import JSONResponse

from .agent.loop import run_agent_loop
from .prompt.optimizer import optimize_chat_body
from .provider import LMStudioError

logger = logging.getLogger("llm_server.gateway")

router = APIRouter()


def _get_runtime(request: Request):
    runtime = getattr(request.app.state, "runtime", None)
    if runtime is None:
        raise HTTPException(status_code=500, detail="runtime 未初始化")
    return runtime


def _make_chat_response(model: str, content: str, tool_calls: list[dict] | None = None,
                        usage: dict | None = None, finish: str = "stop") -> dict:
    return {
        "id": f"chatcmpl-{uuid.uuid4().hex[:12]}",
        "object": "chat.completion",
        "created": int(time.time()),
        "model": model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": content,
                "tool_calls": tool_calls or [],
            },
            "finish_reason": finish,
        }],
        "usage": usage or {},
    }


def _upstream_error(e: Exception) -> HTTPException:
    return HTTPException(status_code=503, detail={
        "error": {
            "message": str(e),
            "type": "upstream_unavailable",
            "hint": "请确认 LM Studio 已启动并已加载模型（默认 http://localhost:11223/v1）。",
        }
    })


# ---------- 健康检查 ----------
@router.get("/healthz")
async def healthz(request: Request):
    runtime = _get_runtime(request)
    upstream = await runtime.provider.ping()
    return {
        "status": "ok" if upstream.get("ok") else "degraded",
        "service": "llm_server",
        "upstream": upstream,
        "tools": len(runtime.tools.list()),
        "rrserver": runtime.registrar.status(),
    }


@router.get("/rr/heartbeat")
async def rr_heartbeat(request: Request):
    """rrserver 心跳探活端点。

    以 `transport=http` 注册时，云端 40 分钟没收到心跳会来访问此端点；
    1 分钟内没有回应或回应非 2xx，云端会记录日志并注销本条注册维护。
    """
    runtime = _get_runtime(request)
    return {
        "status": "ok",
        "service": "llm_server",
        "rrserver": runtime.registrar.status(),
    }


# ---------- OpenAI 兼容透传 ----------
@router.get("/v1/models")
async def list_models(request: Request):
    runtime = _get_runtime(request)
    try:
        models = await runtime.provider.list_models()
    except LMStudioError as e:
        raise _upstream_error(e)
    return {"object": "list", "data": models}


@router.post("/v1/chat/completions")
async def chat_completions(body: dict[str, Any], request: Request):
    runtime = _get_runtime(request)
    agent_mode = (
        request.headers.get("x-tcm-agent", "").lower() in ("1", "true", "yes")
        or body.pop("agent", False) is True
    )

    # 网关内 agent 模式：本服务自主完成 prompt 优化 + tool calling 循环
    if agent_mode:
        return await _run_agent_as_chat(runtime, body)

    # 普通模式：prompt 优化后透传 LM Studio
    optimized, stats = optimize_chat_body(body, runtime.cfg) \
        if runtime.cfg.enable_prompt_optimize else (body, {})
    if stats.get("clipped") or stats.get("merged"):
        logger.info("prompt 优化: %s", stats)
    try:
        return await runtime.provider.chat_completions(optimized)
    except LMStudioError as e:
        raise _upstream_error(e)


@router.post("/v1/responses")
async def responses(body: dict[str, Any], request: Request):
    runtime = _get_runtime(request)
    try:
        return await runtime.provider.responses(body)
    except LMStudioError as e:
        raise _upstream_error(e)


@router.post("/v1/embeddings")
async def embeddings(body: dict[str, Any], request: Request):
    runtime = _get_runtime(request)
    try:
        return await runtime.provider.embeddings(body)
    except LMStudioError as e:
        raise _upstream_error(e)


# ---------- Agent ----------
async def _run_agent_as_chat(runtime, body: dict[str, Any]) -> JSONResponse:
    """把 agent 循环结果包装成 OpenAI chat.completion 响应。"""
    messages = body.get("messages") or []
    if not messages:
        raise HTTPException(status_code=400, detail="messages 不能为空")
    model = body.get("model") or runtime.cfg.default_model
    tools = body.get("tools")
    result = await run_agent_loop(
        runtime.provider, runtime.tools, runtime.cfg,
        messages=messages, model=model, tools=tools,
        max_rounds=body.get("max_rounds"),
        temperature=body.get("temperature", 0.4),
    )
    resp = _make_chat_response(model, result.content, usage=result.usage)
    resp["rounds"] = result.rounds
    resp["trace"] = result.trace
    return JSONResponse(content=resp)


@router.post("/v1/agent/run")
async def agent_run(body: dict[str, Any], request: Request):
    """完整 Agent 接口：prompt 优化 → MCP 工具 → tool calling 循环。

    body:
      model:     模型名（默认 DEFAULT_MODEL）
      messages: 必填，对话消息（含 system/user 等）
      system:   可选，附加/覆盖的 system 提示
      tools:    可选，额外 OpenAI 工具声明（执行依赖注册表/MCP，未注册会提示模型）
      max_rounds: 可选，覆盖最大轮数
      temperature: 可选
    """
    runtime = _get_runtime(request)
    messages = list(body.get("messages") or [])
    if not messages:
        raise HTTPException(status_code=400, detail="messages 不能为空")

    if extra_system := body.get("system"):
        messages.insert(0, {"role": "system", "content": extra_system})
    model = body.get("model") or runtime.cfg.default_model
    tools = body.get("tools")

    result = await run_agent_loop(
        runtime.provider, runtime.tools, runtime.cfg,
        messages=messages, model=model, tools=tools,
        max_rounds=body.get("max_rounds"),
        temperature=body.get("temperature", 0.4),
    )
    return {
        "object": "agent.run",
        "model": model,
        "content": result.content,
        "rounds": result.rounds,
        "trace": result.trace,
        "usage": result.usage,
    }


@router.get("/v1/agent/tools")
async def agent_tools(request: Request):
    runtime = _get_runtime(request)
    return {"object": "list", "tools": runtime.tools.schemas()}


# 兜底：未命中路由的统一 JSON 错误
@router.api_route("/{path:path}", methods=["GET", "POST", "PUT", "DELETE", "OPTIONS"])
async def fallback(path: str):
    raise HTTPException(status_code=404, detail=f"未找到端点: /{path}")
