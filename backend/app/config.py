"""全局配置：加载 routing.yaml，提供 Sub-Agent 路由与 LLM 配置。"""
from __future__ import annotations

import os
from pathlib import Path
from typing import Any

import yaml

BASE_DIR = Path(__file__).resolve().parent
# 路由配置文件路径；部署 llm_server 时可用 TCM_ROUTING_FILE 切换到 LLM 路由
ROUTING_FILE = Path(os.environ.get("TCM_ROUTING_FILE", str(BASE_DIR / "routing.yaml")))
UPLOAD_DIR = BASE_DIR.parent / "uploads"
UPLOAD_DIR.mkdir(exist_ok=True)
SKILLS_DIR = Path(os.environ.get("TCM_SKILLS_DIR", str(BASE_DIR / "skills")))

_DEFAULTS: dict[str, Any] = {
    "routing": {},
    "llm": {"base_url": "", "api_key_env": "TCM_LLM_API_KEY", "timeout": 60,
            "models": {}, "adaptive_route": False, "light_model": "text-default",
            "light_threshold_evidences": 3, "light_threshold_round": 1},
    "loop": {"max_rounds": 8, "single_conf": 0.55, "single_gap": 0.15,
             "dual_conf": 0.42, "min_evidences": 5},
    # MCP：本项目既作为 MCP Server 暴露中医能力，也作为 MCP Client 接入外部工具
    "mcp": {
        "server": {"enabled": True, "mount_path": "/mcp",
                   "expose_agent_tools": True, "expose_session_tools": True},
        "clients": [],
        "call_timeout": 30,
    },
}


class Settings:
    def __init__(self) -> None:
        data: dict[str, Any] = dict(_DEFAULTS)
        if ROUTING_FILE.exists():
            loaded = yaml.safe_load(ROUTING_FILE.read_text(encoding="utf-8")) or {}
            for k, v in loaded.items():
                data[k] = v
        self.routing: dict[str, dict] = data.get("routing") or {}
        self.llm: dict[str, Any] = {**_DEFAULTS["llm"], **(data.get("llm") or {})}
        self.loop: dict[str, Any] = {**_DEFAULTS["loop"], **(data.get("loop") or {})}
        self.mcp: dict[str, Any] = self._merge_mcp(data.get("mcp") or {})

        # 环境变量覆盖（容器/编排场景优先于 routing.yaml）
        env_base = os.environ.get("TCM_LLM_BASE_URL")
        if env_base:
            self.llm["base_url"] = env_base
        env_provider = os.environ.get("TCM_LLM_PROVIDER")
        if env_provider:
            self.llm["provider"] = env_provider
        env_api = os.environ.get("TCM_LLM_API")
        if env_api:
            self.llm["api"] = env_api
        models = self.llm.setdefault("models", {})
        t = os.environ.get("TCM_LLM_TEXT_MODEL")
        if t:
            models["text-default"] = t
        v = os.environ.get("TCM_LLM_VISION_MODEL")
        if v:
            models["vision-default"] = v
        # 视觉模型（望诊）可独立部署为 Qwen3-VL 实例，单独指定端点；
        # 配置后 vision-default 变为 {"model": ..., "base_url": ...}。
        v_url = os.environ.get("TCM_LLM_VISION_BASE_URL")
        if v_url:
            cur = models.get("vision-default", "Qwen3-VL-8B")
            cur = cur if isinstance(cur, dict) else {"model": cur}
            cur["base_url"] = v_url
            models["vision-default"] = cur

        self.host = os.environ.get("TCM_HOST", "0.0.0.0")
        self.port = int(os.environ.get("TCM_PORT", "8000"))
        cors = os.environ.get("TCM_CORS_ORIGINS")
        self.cors_origins = cors.split(",") if cors else ["*"]

    # ---- MCP ----
    @staticmethod
    def _merge_mcp(loaded: dict) -> dict[str, Any]:
        """合并 mcp 配置并应用环境变量覆盖。

        环境变量：
        - TCM_MCP_SERVER_ENABLED : "0"/"false" 关闭 /mcp 挂载
        - TCM_MCP_MOUNT_PATH     : 修改挂载路径
        - TCM_MCP_CLIENTS        : JSON 数组，覆盖 clients 列表
        """
        base = _DEFAULTS["mcp"]
        server = {**base["server"], **((loaded.get("server") or {}) if isinstance(loaded, dict) else {})}
        clients = list(loaded.get("clients") or []) if isinstance(loaded, dict) else []
        timeout = (loaded.get("call_timeout") if isinstance(loaded, dict) else None) or base["call_timeout"]

        env_enabled = os.environ.get("TCM_MCP_SERVER_ENABLED")
        if env_enabled is not None:
            server["enabled"] = env_enabled.strip().lower() not in ("0", "false", "no", "")
        env_path = os.environ.get("TCM_MCP_MOUNT_PATH")
        if env_path:
            server["mount_path"] = env_path
        env_clients = os.environ.get("TCM_MCP_CLIENTS")
        if env_clients:
            import json as _json
            try:
                parsed = _json.loads(env_clients)
                if isinstance(parsed, list):
                    clients = parsed
            except ValueError:
                pass
        env_timeout = os.environ.get("TCM_MCP_CALL_TIMEOUT")
        if env_timeout:
            try:
                timeout = float(env_timeout)
            except ValueError:
                pass
        return {"server": server, "clients": clients, "call_timeout": float(timeout)}

    @property
    def mcp_server_enabled(self) -> bool:
        return bool(self.mcp["server"].get("enabled", True))

    @property
    def mcp_mount_path(self) -> str:
        path = str(self.mcp["server"].get("mount_path") or "/mcp")
        return path if path.startswith("/") else f"/{path}"

    @property
    def mcp_call_timeout(self) -> float:
        return float(self.mcp.get("call_timeout", 30))

    def mcp_client_configs(self, only_enabled: bool = True) -> list[dict]:
        """返回启动时需要自动连接的外部 MCP Server 配置列表。"""
        out = []
        for item in self.mcp.get("clients") or []:
            if not isinstance(item, dict) or not item.get("name"):
                continue
            if only_enabled and not item.get("enabled", True):
                continue
            out.append(dict(item))
        return out

    # ---- Sub-Agent 路由 ----
    def route_of(self, capability: str) -> dict[str, Any]:
        """返回某 capability 的路由配置 {impl, model, options}。"""
        cfg = self.routing.get(capability) or {}
        return {
            "impl": cfg.get("impl", "rule"),
            "model": cfg.get("model", "text-default"),
            "options": cfg.get("options") or {},
        }

    # ---- LLM ----
    @property
    def llm_api_key(self) -> str:
        return os.environ.get(self.llm.get("api_key_env", "TCM_LLM_API_KEY"), "")

    def resolve_model(self, logical_name: str) -> str:
        entry = (self.llm.get("models") or {}).get(logical_name, logical_name)
        if isinstance(entry, dict):
            return entry.get("model", logical_name)
        return entry

    def resolve_base_url(self, logical_name: str) -> str:
        """返回某逻辑模型的专属端点；未配置时回退到全局 base_url。

        视觉模型（vision-default）通常单独部署一个 Qwen3-VL 实例，
        通过 TCM_LLM_VISION_BASE_URL 指向它；其余模型走全局 base_url。
        """
        entry = (self.llm.get("models") or {}).get(logical_name)
        if isinstance(entry, dict) and entry.get("base_url"):
            return entry["base_url"]
        return self.llm.get("base_url") or ""


settings = Settings()
