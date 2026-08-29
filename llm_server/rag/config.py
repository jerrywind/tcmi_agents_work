"""llm_server RAG —— 配置与依赖解析。

三种检索模式共用同一套配置：
- 文本 RAG：用 Embedding 端点把文本片段向量化后最近邻检索；
- 图像 RAG：用 Qwen3-VL 给图片生成 caption，再对 caption 做文本向量检索；
- 图文对应 RAG：每条记录同时持有「图像 caption 向量」与「文本描述向量」，
  支持以文搜图 / 以图搜文 / 图文联合检索。
"""
from __future__ import annotations

import os
from dataclasses import dataclass, field
from pathlib import Path


@dataclass
class RAGConfig:
    # ---- 向量/模型服务端点（复用 llm_server 网关的 OpenAI 兼容 API）----
    # 本地单独跑 RAG 时，将下面两个 base_url 改为 http://localhost:8000/v1
    embed_base_url: str = "http://llm_server:8000/v1"
    embed_api_key: str = "sk-noauth"
    embed_model: str = "text-embedding-default"      # 文本 Embedding 模型名（需 LM Studio 加载 embedding 模型）
    vision_base_url: str = "http://llm_server:8000/v1"
    vision_api_key: str = "sk-noauth"
    vision_model: str = "google/gemma-4-12b-qat"     # 图像 caption 用的多模态模型（走网关透传 LM Studio）

    # ---- 检索参数 ----
    top_k: int = 5
    score_threshold: float = 0.0
    dim: int = 0                                     # 0 = 首次嵌入时自动探测

    # ---- 存储 ----
    # 注意：这里用**字面量默认值**而非 `field(default_factory=...)`。
    # default_factory 会让它没有类属性，`from_env()` 里的 `cls.xxx` 取默认值时会
    # 抛 AttributeError（此前 `RAGConfig.from_env()` 因此完全不可用）。
    # Path 是不可变对象，用作类级默认值是安全的。
    data_dir: Path = Path("/data/rag")
    index_name: str = "tcm"

    # ---- 资源加载 ----
    # 支持形如 images/<id>.jpg + images/<id>.txt（配对）或 texts/<file>.txt/.md
    corpus_dir: Path | None = None

    # ---- 典籍语料索引（T4.3，见 corpus.py）----
    # 由 `python -m rag corpus-build` 生成；配置后文本检索会把它作为
    # 向量检索之外的补充（端点不可达时它就是唯一的检索路径）。
    corpus_db: Path = Path("/data/rag/corpus.sqlite3")
    # 片段切分参数
    corpus_max_chars: int = 600
    corpus_overlap: int = 80
    # 书级召回部数：命中越多越全，但每多一部就要多读一本书做精排
    corpus_top_docs: int = 3

    @classmethod
    def from_env(cls) -> "RAGConfig":
        def env(name: str, default: str) -> str:
            return os.environ.get(name, default)

        return cls(
            embed_base_url=env("RAG_EMBED_BASE_URL", cls.embed_base_url),
            embed_api_key=env("RAG_EMBED_API_KEY", cls.embed_api_key),
            embed_model=env("RAG_EMBED_MODEL", cls.embed_model),
            vision_base_url=env("RAG_VISION_BASE_URL", cls.vision_base_url),
            vision_api_key=env("RAG_VISION_API_KEY", cls.vision_api_key),
            vision_model=env("RAG_VISION_MODEL", cls.vision_model),
            top_k=int(env("RAG_TOP_K", str(cls.top_k))),
            score_threshold=float(env("RAG_SCORE_THRESHOLD", str(cls.score_threshold))),
            dim=int(env("RAG_DIM", str(cls.dim))),
            data_dir=Path(env("RAG_DATA_DIR", str(cls.data_dir))),
            index_name=env("RAG_INDEX_NAME", cls.index_name),
            corpus_dir=Path(env("RAG_CORPUS_DIR")) if env("RAG_CORPUS_DIR", "") else None,
            corpus_db=Path(env("RAG_CORPUS_DB", str(cls.corpus_db))),
            corpus_max_chars=int(env("RAG_CORPUS_MAX_CHARS", str(cls.corpus_max_chars))),
            corpus_overlap=int(env("RAG_CORPUS_OVERLAP", str(cls.corpus_overlap))),
            corpus_top_docs=int(env("RAG_CORPUS_TOP_DOCS", str(cls.corpus_top_docs))),
        )

    # ---- 持久化文件路径 ----
    def index_path(self) -> Path:
        self.data_dir.mkdir(parents=True, exist_ok=True)
        return self.data_dir / f"{self.index_name}.rag.json"

    def images_dir(self) -> Path:
        return self.corpus_dir or (self.data_dir / "images")

    def texts_dir(self) -> Path:
        return self.corpus_dir or (self.data_dir / "texts")
