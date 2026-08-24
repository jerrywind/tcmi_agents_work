"""验证「需要图像理解的能力」路由到独立的 Qwen3-VL 视觉端点。

设计要点：
- 文本能力（text-default）走全局 TCM_LLM_BASE_URL（qwen3.6-9B）。
- 视觉能力（vision-default）可单独部署 Qwen3-VL，经 TCM_LLM_VISION_BASE_URL 指向专属端点，
  OpenAICompatProvider.chat 会按模型名解析端点，从而透明路由。
"""
from __future__ import annotations

import pytest

from app.config import Settings
from app.protocol import llm as llm_mod
from app.protocol.llm import (
    MockProvider,
    OpenAICompatProvider,
    get_provider_for_model,
)


def test_vision_model_env_wiring(monkeypatch):
    monkeypatch.setenv("TCM_LLM_VISION_MODEL", "Qwen3-VL-8B")
    monkeypatch.setenv("TCM_LLM_VISION_BASE_URL", "http://llm_vision:8000/v1")
    s = Settings()
    assert s.resolve_model("vision-default") == "Qwen3-VL-8B"
    assert s.resolve_base_url("vision-default") == "http://llm_vision:8000/v1"
    # 文本模型仍回退到全局 base_url（此处未设置 -> 空串）
    assert s.resolve_base_url("text-default") == (s.llm.get("base_url") or "")


def test_vision_falls_back_to_global_when_no_override():
    s = Settings()
    assert s.resolve_base_url("vision-default") == (s.llm.get("base_url") or "")


def test_get_provider_for_model_routing(monkeypatch):
    monkeypatch.setenv("TCM_LLM_API_KEY", "sk-x")
    monkeypatch.setenv("TCM_LLM_BASE_URL", "http://text:8000/v1")
    monkeypatch.setenv("TCM_LLM_VISION_MODEL", "Qwen3-VL-8B")
    monkeypatch.setenv("TCM_LLM_VISION_BASE_URL", "http://vision:8000/v1")
    s = Settings()
    monkeypatch.setattr(llm_mod, "settings", s)

    p = get_provider_for_model("vision-default")
    assert isinstance(p, OpenAICompatProvider)
    assert p.base_url == "http://vision:8000/v1"

    # 无专属端点 / 无 key 时回退 Mock（保持离线可用）
    monkeypatch.setenv("TCM_LLM_API_KEY", "")
    s2 = Settings()
    monkeypatch.setattr(llm_mod, "settings", s2)
    assert isinstance(get_provider_for_model("vision-default"), MockProvider)


@pytest.mark.asyncio
async def test_provider_chat_routes_per_model_endpoint(monkeypatch):
    captured: dict = {}

    class _Resp:
        def raise_for_status(self):
            return None

        def json(self):
            return {"choices": [{"message": {"content": "ok"}}]}

    class _Client:
        def __init__(self, *a, **k):
            pass

        async def __aenter__(self):
            return self

        async def __aexit__(self, *a):
            return False

        async def post(self, url, **k):
            captured["url"] = url
            captured["body"] = k.get("json")
            return _Resp()

    monkeypatch.setattr("app.protocol.llm.httpx.AsyncClient", _Client)

    s = Settings()
    s.llm["base_url"] = "http://text:8000/v1"
    s.llm["models"]["text-default"] = "qwen3.6-9B"
    s.llm["models"]["vision-default"] = {
        "model": "Qwen3-VL-8B",
        "base_url": "http://vision:8000/v1",
    }
    monkeypatch.setattr(llm_mod, "settings", s)

    p = OpenAICompatProvider("http://text:8000/v1", "sk-test")

    await p.chat([{"role": "user", "content": "描述舌象"}], "vision-default")
    assert captured["url"].startswith("http://vision:8000/v1")
    assert captured["body"]["model"] == "Qwen3-VL-8B"

    captured.clear()
    await p.chat([{"role": "user", "content": "你好"}], "text-default")
    assert captured["url"].startswith("http://text:8000/v1")
    assert captured["body"]["model"] == "qwen3.6-9B"
