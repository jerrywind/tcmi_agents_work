"""RAGConfig.from_env 的解析测试（离线、无网络）。

回归点：`corpus_dir=Path(env("RAG_CORPUS_DIR")) if env(...) else None` 曾少传
`default` 参数——只要配置了 `RAG_CORPUS_DIR`（容器部署标配）就走真分支抛
TypeError，导致 llm_server 启动时 RAG 整体挂载失败、静默降级成 503。
配置了语料却从不报错到「从头到尾 RAG 都是空的」，是同一类静默故障。
"""
from __future__ import annotations

import os
import unittest
from pathlib import Path
from unittest.mock import patch

try:
    from .config import RAGConfig
except ImportError:  # 直接 `python -m unittest test_config` 时无包上下文
    from config import RAGConfig


class TestFromEnv(unittest.TestCase):
    def test_no_env_uses_defaults(self):
        with patch.dict(os.environ, {}, clear=True):
            cfg = RAGConfig.from_env()
        self.assertIsNone(cfg.corpus_dir)
        self.assertEqual(cfg.corpus_db, Path("/data/rag/corpus.sqlite3"))
        self.assertEqual(cfg.data_dir, Path("/data/rag"))
        self.assertEqual(cfg.top_k, 5)

    def test_corpus_env_when_set(self):
        """配置 RAG_CORPUS_DIR / RAG_CORPUS_DB（容器部署标配）不得抛错。"""
        with patch.dict(
            os.environ,
            {
                "RAG_CORPUS_DIR": "/data/rag",
                "RAG_CORPUS_DB": "/data/rag/_index/corpus.sqlite3",
                "RAG_DATA_DIR": "/data/rag",
                "RAG_TOP_K": "3",
            },
            clear=True,
        ):
            cfg = RAGConfig.from_env()
        self.assertEqual(cfg.corpus_dir, Path("/data/rag"))
        self.assertEqual(cfg.corpus_db, Path("/data/rag/_index/corpus.sqlite3"))
        self.assertEqual(cfg.top_k, 3)

    def test_corpus_env_blank_yields_none(self):
        with patch.dict(os.environ, {"RAG_CORPUS_DIR": "  ", "RAG_CORPUS_DB": ""}, clear=True):
            cfg = RAGConfig.from_env()
        self.assertIsNone(cfg.corpus_dir)
        self.assertEqual(cfg.corpus_db, Path("/data/rag/corpus.sqlite3"))


if __name__ == "__main__":
    unittest.main()
