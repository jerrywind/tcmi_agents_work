#!/usr/bin/env python3
"""OpenAI 兼容 mock，替代真实 llm_server 用于 rrserver 隧道联调。

真实 llm_server 加载 GGUF 权重、需要 GPU/大模型，本地无法常驻。本 mock 暴露
**完全相同的 OpenAI 兼容契约**（`/v1/chat/completions` + `/v1/models`），使「backend
→ rrserver 隧道 → 本地 llm 服务」这条生产链路可以被真实驱动验证：

  * `stream: false` → 返回标准 JSON `chat.completion`（backend 的
    OpenAICompatProvider 实际消费的形状）；
  * `stream: true`  → 返回 `text/event-stream` SSE，按字切片逐片吐出，
    用于验证隧道「真·流式」透传（首字延迟不受响应总长影响）。

运行：python mock_llm_server.py [port]   （默认 19090）
"""
from __future__ import annotations

import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 19090

TEXT = (
    "辨证：患者舌淡红、苔薄白，脉浮数，属风热犯肺之证；"
    "治法：疏风清热、宣肺止咳；方药：桑菊饮加减（桑叶、菊花、薄荷、"
    "杏仁、桔梗、甘草），忌辛辣油腻，多饮水、避风寒。"
)


def sse_events(text: str):
    """把回答按字切片，模拟 LLM 逐 token 增量输出。"""
    for ch in text:
        yield {
            "id": "cmpl-mock",
            "object": "chat.completion.chunk",
            "choices": [{"index": 0, "delta": {"content": ch}, "finish_reason": None}],
        }
    yield {
        "id": "cmpl-mock",
        "object": "chat.completion.chunk",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
    }


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"  # 必须 1.1 才能用 chunked 做真流式

    def log_message(self, *args):  # 静默
        pass

    def _send_json(self, obj, status=200):
        body = json.dumps(obj, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path in ("/health", "/"):
            self._send_json({"status": "ok"})
        elif self.path == "/v1/models":
            self._send_json(
                {"object": "list", "data": [{"id": "text-default", "object": "model"}]}
            )
        else:
            self._send_json({"error": "not found"}, 404)

    def do_POST(self):
        if self.path != "/v1/chat/completions":
            self._send_json({"error": "not found"}, 404)
            return
        length = int(self.headers.get("Content-Length", 0))
        req = json.loads(self.rfile.read(length) or b"{}")
        model = req.get("model", "text-default")
        stream = bool(req.get("stream", False))

        if not stream:
            self._send_json(
                {
                    "id": "cmpl-mock",
                    "object": "chat.completion",
                    "created": int(time.time()),
                    "model": model,
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": TEXT},
                            "finish_reason": "stop",
                        }
                    ],
                    "usage": {
                        "prompt_tokens": 12,
                        "completion_tokens": len(TEXT),
                        "total_tokens": 12 + len(TEXT),
                    },
                }
            )
            return

        # ---- 真·流式：HTTP/1.1 chunked 编码，逐片 flush ----
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream; charset=utf-8")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()

        def emit(payload: str):
            data = payload.encode("utf-8")
            self.wfile.write(("%X\r\n" % len(data)).encode())
            self.wfile.write(data)
            self.wfile.write(b"\r\n")
            self.wfile.flush()

        for ev in sse_events(TEXT):
            emit("data: " + json.dumps(ev, ensure_ascii=False) + "\n\n")
            time.sleep(0.02)  # 制造可观测的分片间隔
        emit("data: [DONE]\n\n")
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()


if __name__ == "__main__":
    srv = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    print(f"mock llm_server listening on 127.0.0.1:{PORT}", flush=True)
    srv.serve_forever()
