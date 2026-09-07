"""RAGService 的离线回归（不依赖索引、不联网）。

回归点：向量库为空时 `retrieve_text` 仍然去打 embedding 端点。
库里一条向量都没有，嵌入结果无处可用；而离线部署的常态就是端点不可达，
每请求要白付数秒（实测 8s），叠加首次读盘后整次检索近 30s，
足以顶穿调用方超时——harness 探活因此把「慢」判成「挂」。
"""
from __future__ import annotations

import asyncio
import tempfile
import unittest
from pathlib import Path

try:
    from .config import RAGConfig
    from .retriever import RAGService
    from .store import Record
except ImportError:  # 直接 `python -m unittest test_retriever` 时无包上下文
    from config import RAGConfig
    from retriever import RAGService
    from store import Record


class _RecordingEmbedder:
    """只记次数的假 embedder：真端点不可达时会拖慢数秒，测试里不能真打。"""

    def __init__(self) -> None:
        self.calls = 0

    async def embed_one(self, text: str) -> list[float]:
        self.calls += 1
        return [1.0, 0.0]


def _service_without_corpus() -> RAGService:
    return RAGService(RAGConfig(
        data_dir=Path(tempfile.mkdtemp()),
        corpus_db=Path("/nonexistent/corpus.sqlite3"),
    ))


class TestEmptyStoreSkipsEmbedding(unittest.TestCase):
    def test_empty_store_does_not_embed(self):
        svc = _service_without_corpus()
        emb = _RecordingEmbedder()
        svc.text_embedder = emb
        asyncio.run(svc.retrieve_text("咳嗽"))
        self.assertEqual(emb.calls, 0, "空向量库时不该调用 embedding 端点")

    def test_non_empty_store_still_embeds(self):
        """空库跳过是**短路**，不是把嵌入整条路废掉。"""
        svc = _service_without_corpus()
        emb = _RecordingEmbedder()
        svc.text_embedder = emb
        svc.store.add(Record(id="a", text="咳嗽", text_vec=[1.0, 0.0]))
        asyncio.run(svc.retrieve_text("咳嗽"))
        self.assertEqual(emb.calls, 1, "库里有向量时必须照常嵌入")

    def test_paired_and_image_skip_when_empty(self):
        svc = _service_without_corpus()
        self.assertEqual(asyncio.run(svc.retrieve_paired(query="咳嗽")), [])
        self.assertEqual(asyncio.run(svc.retrieve_image("/no/such.jpg")), [])

    def test_warmup_without_corpus_is_noop(self):
        """没索引时预热必须静默返回，不能把启动搞挂。"""
        svc = _service_without_corpus()
        asyncio.run(svc.warmup())


if __name__ == "__main__":
    unittest.main()
