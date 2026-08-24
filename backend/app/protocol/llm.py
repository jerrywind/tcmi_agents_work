"""LLM Provider 抽象：openai 兼容协议 + mock 降级。

Sub-Agent 的 LLM 实现只依赖 LLMProvider 接口，不关心具体厂商；
换模型 = 改 routing.yaml 的 models 映射或 capability 级 model 字段。
"""
from __future__ import annotations

import base64
import json
from abc import ABC, abstractmethod
from pathlib import Path

import httpx

from ..config import settings


class LLMProvider(ABC):
    """对话接口。

    ``tools`` 为 None 时返回 ``(字符串, usage_dict)``；传入工具声明时返回
    ``({"content": str, "tool_calls": [...]}, usage_dict)`` 元组，供 function-calling
    循环使用。``usage_dict`` 形如 ``{"prompt_tokens": int, "completion_tokens": int,
    "total_tokens": int}``，用于可观测性（trace 中的 token 用量）。
    """

    @abstractmethod
    async def chat(self, messages: list[dict], model: str, json_mode: bool = False,
                   tools: list[dict] | None = None) -> tuple[str | dict, dict]:
        ...

    @property
    def name(self) -> str:
        return type(self).__name__


class MockProvider(LLMProvider):
    """无 API Key 时的降级实现：返回空结果，让调用方走规则兜底。"""

    async def chat(self, messages: list[dict], model: str, json_mode: bool = False,
                   tools: list[dict] | None = None) -> tuple[str | dict, dict]:
        if tools:
            # 工具模式：返回空工具调用，结束工具循环
            return {"content": "{}", "tool_calls": []}, {}
        return ("{}" if json_mode else ""), {}


class OpenAICompatProvider(LLMProvider):
    def __init__(self, base_url: str, api_key: str, timeout: int = 60) -> None:
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self.timeout = timeout
        # api: "chat"(默认, /chat/completions) 或 "responses"(/responses, LM Studio Responses API)
        self.api = (settings.llm.get("api") or "chat").lower()

    async def chat(self, messages: list[dict], model: str, json_mode: bool = False,
                   tools: list[dict] | None = None) -> tuple[str | dict, dict]:
        # 每个逻辑模型可有专属端点：视觉模型（Qwen3-VL）独立部署时，
        # 这里按模型名解析出对应 base_url，从而透明地路由到不同实例。
        base = settings.resolve_base_url(model) or self.base_url
        if self.api == "responses":
            return await self._chat_responses(base, messages, model, json_mode, tools)
        return await self._chat_chat(base, messages, model, json_mode, tools)

    # ---------- Chat Completions API ----------
    async def _chat_chat(self, base: str, messages: list[dict], model: str,
                         json_mode: bool, tools: list[dict] | None) -> tuple[str | dict, dict]:
        body: dict = {"model": settings.resolve_model(model), "messages": messages}
        if tools:
            body["tools"] = tools
            body["tool_choice"] = "auto"
        elif json_mode:
            body["response_format"] = {"type": "json_object"}
        async with httpx.AsyncClient(timeout=self.timeout) as client:
            r = await client.post(
                f"{base}/chat/completions",
                headers={"Authorization": f"Bearer {self.api_key}"},
                json=body,
            )
            r.raise_for_status()
            payload = r.json()
            usage = payload.get("usage") or {}
            msg = payload["choices"][0]["message"]
            if tools:
                return ({"content": msg.get("content") or "",
                         "tool_calls": msg.get("tool_calls") or []}, usage)
            return (msg["content"], usage)

    # ---------- Responses API (LM Studio) ----------
    @staticmethod
    def _to_responses_input(messages: list[dict]) -> list[dict]:
        """将 chat messages 转换为 Responses API 的 input 项列表。

        支持 system/user/assistant/tool 角色，及 user 消息中的多模态
        content（text / image_url）。tool 消息转换为 function_call_output。
        """
        items: list[dict] = []
        for m in messages:
            role = m.get("role")
            content = m.get("content")
            if role == "system":
                items.append({"role": "system", "content": content or ""})
            elif role == "user":
                items.append({"role": "user", "content": _conv_content(content)})
            elif role == "assistant":
                # assistant 可能带有 tool_calls（工具轮次产物）
                tcs = m.get("tool_calls")
                if tcs:
                    for tc in tcs:
                        fn = tc.get("function", {})
                        items.append({
                            "type": "function_call",
                            "call_id": tc.get("id", ""),
                            "name": fn.get("name", ""),
                            "arguments": fn.get("arguments", "") or "{}",
                        })
                if content:
                    items.append({"role": "assistant", "content": content})
            elif role == "tool":
                items.append({
                    "type": "function_call_output",
                    "call_id": m.get("tool_call_id", ""),
                    "output": content if isinstance(content, str) else json.dumps(content, ensure_ascii=False),
                })
        return items

    async def _chat_responses(self, base: str, messages: list[dict], model: str,
                              json_mode: bool, tools: list[dict] | None) -> tuple[str | dict, dict]:
        import uuid
        body: dict = {
            "model": settings.resolve_model(model),
            "input": self._to_responses_input(messages),
        }
        if tools:
            body["tools"] = [{
                "type": "function",
                "name": t["function"]["name"],
                "description": t["function"].get("description", ""),
                "parameters": t["function"].get("parameters", {}),
            } for t in tools]
        elif json_mode:
            body["text"] = {"format": {"type": "json_object"}}

        async with httpx.AsyncClient(timeout=self.timeout) as client:
            r = await client.post(
                f"{base}/responses",
                headers={"Authorization": f"Bearer {self.api_key}"},
                json=body,
            )
            r.raise_for_status()
            payload = r.json()
            usage = payload.get("usage") or {}

        # 解析输出：提取文本 + 工具调用（function_call 输出项）
        out_text = ""
        tool_calls: list[dict] = []
        for item in payload.get("output", []):
            t = item.get("type")
            if t == "message":
                for c in item.get("content", []):
                    if c.get("type") == "output_text":
                        out_text += c.get("text", "")
            elif t == "function_call":
                tool_calls.append({
                    "id": item.get("call_id") or f"call_{uuid.uuid4().hex[:8]}",
                    "type": "function",
                    "function": {
                        "name": item.get("name", ""),
                        "arguments": item.get("arguments", "{}"),
                    },
                })
        if tools:
            return ({"content": out_text, "tool_calls": tool_calls}, usage)
        return (out_text, usage)


_provider: LLMProvider | None = None


def get_provider() -> LLMProvider:
    """返回当前 LLM provider。

    无 API Key（或未配置 base_url）时回退到 :class:`MockProvider`，由调用方走规则兜底。

    注意：这里不使用模块级缓存，而是每次依据当前 ``settings`` 创建 provider。
    原因有二：(1) provider 无状态、创建成本低；(2) 避免全局缓存带来的跨测试
    污染（某测试设置 API Key 后缓存真实 provider，泄漏到后续依赖「无 Key 回退
    Mock」的用例）。运行期单进程内重复创建的开销可忽略。
    """
    key = settings.llm_api_key
    base = settings.llm.get("base_url") or ""
    if key and base:
        return OpenAICompatProvider(base, key, settings.llm.get("timeout", 60))
    return MockProvider()


def get_provider_for_model(logical_name: str) -> LLMProvider:
    """按模型逻辑名返回 Provider。

    - 该模型配置了专属端点（如独立部署的 Qwen3-VL）且有 API Key -> 用专属端点；
    - 否则回退到全局 get_provider()（单一端点或 Mock 降级）。
    """
    base = settings.resolve_base_url(logical_name)
    key = settings.llm_api_key
    if key and base:
        return OpenAICompatProvider(base, key, settings.llm.get("timeout", 60))
    return get_provider()


def image_content(path: str) -> dict:
    """本地图片 -> openai vision content 块。"""
    data = base64.b64encode(Path(path).read_bytes()).decode()
    suffix = Path(path).suffix.lstrip(".") or "jpeg"
    return {"type": "image_url",
            "image_url": {"url": f"data:image/{suffix};base64,{data}"}}


def parse_json(text: str) -> dict:
    """宽容地从 LLM 输出中提取 JSON。"""
    text = text.strip()
    if text.startswith("```"):
        text = text.strip("`")
        if text.startswith("json"):
            text = text[4:]
    try:
        return json.loads(text)
    except Exception:  # noqa: BLE001
        start, end = text.find("{"), text.rfind("}")
        if 0 <= start < end:
            try:
                return json.loads(text[start:end + 1])
            except Exception:  # noqa: BLE001
                pass
    return {}
