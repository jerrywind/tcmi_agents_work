#!/usr/bin/env python3
"""backend ↔ rrserver 隧道 ↔ 本地 llm 服务的端到端对接联调客户端。

两条验证路径（均走真实隧道，非 mock 隧道逻辑）：

1. 非流式（backend 真实生产代码路径）
   —— 直接用 backend 的 `OpenAICompatProvider`（真实代码）向
      `http://<cloud>/t/home/v1/chat/completions` 发起对话，验证 backend
      的 OpenAI 兼容契约经隧道落到本地 llm 服务并正确回包。

2. 真·流式 SSE
   —— 以 OpenAI streaming 客户端形态发送 `stream: true`，验证隧道把本地
      LLM 的逐 token 增量输出原样透传回外部调用方（首字延迟不受总长影响）。

使用前：先启动 dryrun.ps1 拉起 mock / server / client。
"""
from __future__ import annotations

import asyncio
import json
import os
import sys

CLOUD = os.environ.get("CLOUD", "http://127.0.0.1:8080")
TUNNEL_BASE = f"{CLOUD}/t/home/v1"  # -> /t/home/v1/chat/completions

# 让 backend 的真实 provider 直连隧道基址（等价于生产里
# TCM_LLM_BASE_URL=http://<cloud>/t/home/v1），覆盖路由解析副作用。
# 优先用 BACKEND_DIR 环境变量（CI 里可按需覆盖）；否则相对本脚本定位
# 到仓库的 backend/ 目录（rrserver/dryrun/../../backend）。
BACKEND = os.environ.get("BACKEND_DIR") or os.path.normpath(
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "backend")
)
sys.path.insert(0, BACKEND)


async def test_backend_provider_nonstream():
    import app.protocol.llm as llm_mod
    from app.protocol.llm import OpenAICompatProvider

    llm_mod.settings.resolve_base_url = lambda _name: ""  # 强制回退到 self.base_url
    llm_mod.settings.resolve_model = lambda name: name     # 不重写模型名

    provider = OpenAICompatProvider(base_url=TUNNEL_BASE, api_key="noauth")
    reply = await provider.chat(
        messages=[{"role": "user", "content": "我最近咳嗽、喉咙痛、苔薄白，怎么办？"}],
        model="text-default",
    )
    assert isinstance(reply, str) and reply.strip(), f"空回复: {reply!r}"
    assert "辨证" in reply, f"返回内容不符合预期: {reply!r}"
    print(f"[OK] backend OpenAICompatProvider 经隧道返回:\n     {reply}")


async def test_tunnel_true_streaming():
    import httpx

    body = {
        "model": "text-default",
        "messages": [{"role": "user", "content": "你好，请做个简短辨证示例。"}],
        "stream": True,
    }
    chunks: list[str] = []
    first_at = None
    t0 = asyncio.get_event_loop().time()
    async with httpx.AsyncClient(timeout=30) as c:
        async with c.stream(
            "POST",
            f"{TUNNEL_BASE}/chat/completions",
            json=body,
            headers={"Authorization": "Bearer noauth"},
        ) as r:
            assert r.status_code == 200, f"HTTP {r.status_code}"
            ctype = r.headers.get("content-type", "")
            assert "text/event-stream" in ctype, f"非 SSE 响应: {ctype}"
            async for line in r.aiter_lines():
                if not line:
                    continue
                if not line.startswith("data:"):
                    continue
                payload = line[len("data:"):].strip()
                if payload == "[DONE]":
                    break
                obj = json.loads(payload)
                delta = obj["choices"][0]["delta"].get("content", "")
                if delta:
                    if first_at is None:
                        first_at = asyncio.get_event_loop().time() - t0
                    chunks.append(delta)
    text = "".join(chunks)
    assert chunks, "未收到任何流式分片"
    assert text.startswith("辨证"), f"流式内容异常: {text!r}"
    print(
        f"[OK] 隧道真·流式 SSE: 收到 {len(chunks)} 个增量分片, "
        f"首片延迟≈{first_at*1000:.0f}ms, 拼接前 16 字={text[:16]!r}"
    )


async def main():
    print(f"==> 联调目标: {TUNNEL_BASE}/chat/completions\n")
    await test_backend_provider_nonstream()
    print()
    await test_tunnel_true_streaming()
    print("\n=== DRYRUN PASS: backend → 隧道 → 本地 llm 服务 全链路打通 ===")


if __name__ == "__main__":
    asyncio.run(main())
