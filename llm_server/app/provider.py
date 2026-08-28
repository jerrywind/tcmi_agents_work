"""LM Studio 上游客户端：所有模型推理由 LM Studio 提供。

仅做「协议透传 + 状态探测」，本模块不引入任何模型逻辑；
prompt 优化 / tool calling / agent 在 gateway 与 agent 层实现。
"""
from __future__ import annotations

import logging
from typing import Any

import httpx

from .config import Settings

logger = logging.getLogger("llm_server.provider")


class LMStudioError(RuntimeError):
    """LM Studio 上游不可用或返回错误。"""


class LMStudioClient:
    def __init__(self, cfg: Settings) -> None:
        self.cfg = cfg
        self.base_url = cfg.lmstudio_base_url.rstrip("/")
        self.api_key = cfg.lmstudio_api_key

    # ---------- 基础请求 ----------
    async def _post(self, path: str, body: dict[str, Any],
                    timeout: int | None = None) -> dict[str, Any]:
        try:
            async with httpx.AsyncClient(timeout=timeout or self.cfg.request_timeout) as client:
                r = await client.post(
                    f"{self.base_url}/{path.lstrip('/')}",
                    headers={"Authorization": f"Bearer {self.api_key}"},
                    json=body,
                )
                r.raise_for_status()
                return r.json()
        except httpx.HTTPError as e:
            raise LMStudioError(f"LM Studio 不可达（{self.base_url}/{path}）: {e}") from e

    async def _get(self, path: str, timeout: int | None = None) -> Any:
        try:
            async with httpx.AsyncClient(timeout=timeout or 15) as client:
                r = await client.get(
                    f"{self.base_url}/{path.lstrip('/')}",
                    headers={"Authorization": f"Bearer {self.api_key}"},
                )
                r.raise_for_status()
                return r.json()
        except httpx.HTTPError as e:
            raise LMStudioError(f"LM Studio 不可达（{self.base_url}/{path}）: {e}") from e

    # ---------- OpenAI 兼容端点 ----------
    async def chat_completions(self, body: dict[str, Any]) -> dict[str, Any]:
        """透传 /v1/chat/completions，返回 LM Studio 原始响应。"""
        return await self._post("/chat/completions", body)

    async def responses(self, body: dict[str, Any]) -> dict[str, Any]:
        """透传 /v1/responses（LM Studio Responses API），返回原始响应。"""
        return await self._post("/responses", body)

    async def embeddings(self, body: dict[str, Any]) -> dict[str, Any]:
        """透传 /v1/embeddings（供 RAG 等复用 LM Studio 的 embedding 模型）。"""
        return await self._post("/embeddings", body)

    async def list_models(self) -> list[dict]:
        """拉取 LM Studio 当前已加载/可用的模型列表。"""
        data = await self._get("/models")
        return data.get("data", []) if isinstance(data, dict) else []

    # ---------- 探测 ----------
    async def ping(self) -> dict[str, Any]:
        """健康探测：返回上游可达性信息，失败不抛异常。"""
        try:
            models = await self.list_models()
            names = [m.get("id") for m in models if m.get("id")]
            return {
                "ok": True,
                "base_url": self.base_url,
                "models": names[:10],
                "model_count": len(names),
            }
        except Exception as e:  # noqa: BLE001
            logger.warning("LM Studio ping 失败: %s", e)
            return {"ok": False, "base_url": self.base_url, "error": str(e)}
