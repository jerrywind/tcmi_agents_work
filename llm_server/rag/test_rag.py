"""llm_server RAG 单元测试（离线，无需模型服务）。

覆盖：
- VectorStore 文本/图像/图文检索与关键字回退
- 持久化 save/load 往返
- 图文配对记录双向量域召回
- ingest_image 离线降级（无视觉模型时零向量，但记录与 caption 写入）

运行：python -m unittest test_rag -v   （需 numpy）
"""
from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

try:
    from .store import Hit, Record, VectorStore
    from .retriever import RAGService
    from .config import RAGConfig
except ImportError:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from store import Hit, Record, VectorStore
    from retriever import RAGService
    from config import RAGConfig


def _vec(*vals):
    return list(vals)


def _make_store() -> VectorStore:
    s = VectorStore()
    s.add(Record(id="t1", text="风寒感冒：恶寒重发热轻，无汗，头痛身疼",
                 text_vec=_vec(1.0, 0.0, 0.0)))
    s.add(Record(id="t2", text="风热感冒：发热重恶寒轻，有汗，咽喉肿痛",
                 text_vec=_vec(0.0, 1.0, 0.0)))
    s.add(Record(id="p1", text="舌红苔黄腻，湿热内蕴", image_path="/x/1.jpg",
                 image_caption="舌色红，苔黄腻",
                 text_vec=_vec(0.0, 0.0, 1.0),
                 image_vec=_vec(1.0, 1.0, 0.0)))
    return s


class TestRAG(unittest.TestCase):
    def test_text_search(self):
        s = _make_store()
        hits = s.search(_vec(0.9, 0.1, 0.0), modality="text", top_k=1)
        self.assertTrue(hits)
        self.assertEqual(hits[0].id, "t1")
        self.assertIsInstance(hits[0], Hit)

    def test_image_search(self):
        s = _make_store()
        hits = s.search(_vec(1.0, 0.9, 0.0), modality="image", top_k=1)
        self.assertTrue(hits)
        self.assertEqual(hits[0].id, "p1")
        self.assertEqual(hits[0].modality, "image")

    def test_paired_search_returns_paired_record(self):
        s = _make_store()
        hits = s.search(_vec(0.1, 0.1, 0.9), modality="paired", top_k=3)
        ids = {h.id for h in hits}
        self.assertIn("p1", ids)

    def test_keyword_fallback_when_no_vec(self):
        s = VectorStore()
        s.add(Record(id="k1", text="气虚乏力，少气懒言，舌淡胖有齿痕"))
        hits = s.search(None, modality="text", query_text="气虚乏力齿痕")
        self.assertTrue(hits)
        self.assertEqual(hits[0].id, "k1")
        self.assertGreater(hits[0].score, 0)

    def test_save_load_roundtrip(self):
        s = _make_store()
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "idx.rag.json"
            s.save(p)
            self.assertTrue(p.exists())
            loaded = VectorStore.load(p)
            self.assertEqual(len(loaded.records), len(s.records))
            self.assertEqual(loaded.records[2].image_vec, _vec(1.0, 1.0, 0.0))

    def test_ingest_image_record_offline(self):
        with tempfile.TemporaryDirectory() as d:
            cfg = RAGConfig(embed_base_url="http://127.0.0.1:9/v1",
                            vision_base_url="http://127.0.0.1:9/v1",
                            data_dir=Path(d))
            svc = RAGService(cfg)
            img = Path(d) / "a.jpg"
            img.write_bytes(b"\xff\xd8fake")
            rid = __import__("asyncio").run(
                svc.ingest_image(str(img), caption="舌淡白", text="气血不足"))
            rec = svc.store._by_id[rid]
            self.assertEqual(rec.image_caption, "舌淡白")
            self.assertEqual(rec.text, "气血不足")
            self.assertTrue(cfg.index_path().exists())
            raw = json.loads(cfg.index_path().read_text(encoding="utf-8"))
            self.assertTrue(any(r["id"] == rid for r in raw["records"]))


if __name__ == "__main__":
    unittest.main()
