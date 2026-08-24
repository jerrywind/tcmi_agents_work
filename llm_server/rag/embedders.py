"""嵌入器：文本 Embedding + 图像 caption（Qwen3-VL）。

- 文本：复用 llm_server 的 OpenAI 兼容 /v1/embeddings 端点。
- 图像：调用独立的 Qwen3-VL（vision）端点生成中文 caption，再对 caption 做文本 Embedding，
  从而得到可与文本向量同空间比较的 image_vec（用于图像 RAG 与图文对应 RAG）。
"""
from __future__ import annotations

import base64
from typing import Sequence

import httpx
import numpy as np

try:
    from .config import RAGConfig
except ImportError:  # 作为脚本直接运行时退化为绝对导入
    from config import RAGConfig


class TextEmbedder:
    def __init__(self, cfg: RAGConfig) -> None:
        self.cfg = cfg

    async def embed(self, texts: Sequence[str]) -> list[list[float]]:
        """批量文本嵌入；无端点时返回零向量（保证离线可跑）。"""
        texts = [t for t in texts if t]
        if not texts:
            return []
        try:
            async with httpx.AsyncClient(timeout=60) as client:
                r = await client.post(
                    f"{self.cfg.embed_base_url.rstrip('/')}/embeddings",
                    headers={"Authorization": f"Bearer {self.cfg.embed_api_key}"},
                    json={"model": self.cfg.embed_model, "input": list(texts)},
                )
                r.raise_for_status()
                data = r.json()["data"]
                # 兼容返回按 index 排序或顺序返回两种形式
                data.sort(key=lambda d: d.get("index", 0))
                return [d["embedding"] for d in data]
        except Exception:  # noqa: BLE001  无 Embedding 服务时降级为零向量
            return [list(np.zeros(self.cfg.dim or 1, dtype=np.float32)) for _ in texts]

    async def embed_one(self, text: str) -> list[float]:
        res = await self.embed([text])
        return res[0] if res else []


_VL_SYSTEM = (
    "你是中医影像描述助手。请基于图片给出客观、结构化且富含检索关键词的描述"
    "（包含：部位、颜色、形态、纹理、质地、典型表现与可能的中医意象），使用中文，"
    "不使用 markdown，不输出诊断结论。"
)


class ImageEmbedder:
    """先让 Qwen3-VL 生成图像 caption，再对 caption 做文本 Embedding。"""

    def __init__(self, cfg: RAGConfig, text_embedder: TextEmbedder) -> None:
        self.cfg = cfg
        self.text_embedder = text_embedder

    @staticmethod
    def _b64(path: str) -> str:
        return base64.b64encode(Path(path).read_bytes()).decode("ascii")

    async def caption(self, image_path: str) -> str:
        try:
            async with httpx.AsyncClient(timeout=90) as client:
                r = await client.post(
                    f"{self.cfg.vision_base_url.rstrip('/')}/chat/completions",
                    headers={"Authorization": f"Bearer {self.cfg.vision_api_key}"},
                    json={
                        "model": self.cfg.vision_model,
                        "messages": [
                            {"role": "system", "content": _VL_SYSTEM},
                            {"role": "user", "content": [
                                {"type": "text", "text": "请描述此图，用于中医资料检索。"},
                                {"type": "image_url",
                                 "image_url": {"url": f"data:image/jpeg;base64,{self._b64(image_path)}"}},
                            ]},
                        ],
                    },
                )
                r.raise_for_status()
                return (r.json()["choices"][0]["message"]["content"] or "").strip()
        except Exception as e:  # noqa: BLE001
            return f"[caption 失败: {e}]"

    async def embed_image(self, image_path: str) -> tuple[str, list[float]]:
        cap = await self.caption(image_path)
        vec = await self.text_embedder.embed_one(cap) if cap else []
        return cap, vec
