"""llm_server 配置：全部可通过环境变量覆盖（见 .env.example）。

核心变化：模型不再由本服务托管，而是依赖 LM Studio 的 OpenAI 兼容端点。
"""
from __future__ import annotations

import json
import os
from dataclasses import dataclass, field


def _env(name: str, default: str) -> str:
    return os.environ.get(name, default)


def _env_bool(name: str, default: bool) -> bool:
    val = _env(name, "1" if default else "0").strip().lower()
    return val in ("1", "true", "yes", "on")


def _env_int(name: str, default: int) -> int:
    try:
        return int(_env(name, str(default)))
    except ValueError:
        return default


def _env_json_list(name: str) -> list[dict]:
    """解析形如 [{"name":"..","url":"..","headers":{...}}] 的 JSON 数组。"""
    raw = _env(name, "").strip()
    if not raw:
        return []
    try:
        val = json.loads(raw)
        return val if isinstance(val, list) else []
    except json.JSONDecodeError:
        return []


@dataclass
class Settings:
    # ---- 服务监听 ----
    host: str = "0.0.0.0"
    port: int = 8000

    # ---- LM Studio 上游 ----
    # 容器内访问宿主机 LM Studio 时请设为 http://host.docker.internal:11223/v1
    lmstudio_base_url: str = "http://localhost:11223/v1"
    lmstudio_api_key: str = "sk-noauth"
    default_model: str = "google/gemma-4-12b-qat"
    request_timeout: int = 180          # 本地模型推理较慢，放宽超时

    # ---- Prompt 优化 ----
    enable_prompt_optimize: bool = True  # 是否对 /v1/chat/completions 做上下文优化
    prompt_max_chars: int = 12000        # 优化后消息总预算（约 6K~8K token 量级）
    prompt_system_brief: str = (
        "你是风蓝科技智能中医助手。请使用中文回答，尽量简洁、结构化，"
        "不做超出资料范围的诊断承诺。"
    )

    # ---- Agent 循环 ----
    agent_max_rounds: int = 8            # 单次 agent 最大工具调用轮数
    agent_max_tool_output_chars: int = 3000  # 工具结果回填给模型的最大长度

    # ---- MCP ----
    enable_mcp: bool = True              # 是否连接外部 MCP Server 并拉取其工具
    mcp_clients: list[dict] = field(default_factory=list)  # 见 .env.example 的 MCP_CLIENTS

    @classmethod
    def from_env(cls) -> "Settings":
        return cls(
            host=_env("LLM_HOST", "0.0.0.0"),
            port=_env_int("LLM_PORT", 8000),
            lmstudio_base_url=_env("LMSTUDIO_BASE_URL", "http://localhost:11223/v1"),
            lmstudio_api_key=_env("LMSTUDIO_API_KEY", "sk-noauth"),
            default_model=_env("DEFAULT_MODEL", "google/gemma-4-12b-qat"),
            request_timeout=_env_int("REQUEST_TIMEOUT", 180),
            enable_prompt_optimize=_env_bool("ENABLE_PROMPT_OPTIMIZE", True),
            prompt_max_chars=_env_int("PROMPT_MAX_CHARS", 12000),
            prompt_system_brief=_env(
                "PROMPT_SYSTEM_BRIEF",
                "你是风蓝科技智能中医助手。请使用中文回答，尽量简洁、结构化，"
                "不做超出资料范围的诊断承诺。",
            ),
            agent_max_rounds=_env_int("AGENT_MAX_ROUNDS", 8),
            agent_max_tool_output_chars=_env_int("AGENT_MAX_TOOL_OUTPUT_CHARS", 3000),
            enable_mcp=_env_bool("ENABLE_MCP", True),
            mcp_clients=_env_json_list("MCP_CLIENTS"),
        )


settings = Settings.from_env()
