"""检索器：封装三种 RAG 模式。

- text   ：文本 -> Embedding -> 文本向量域最近邻
- image  ：图像 -> Qwen3-VL caption -> Embedding -> 图像向量域最近邻
- paired ：图文联合——可用文本或图像查询，跨 text_vec / image_vec 两个域召回，
           并按查询模态与文档模态给出 modality 标记（以文搜图 / 以图搜文）。
"""
from __future__ import annotations

from pathlib import Path
from typing import Any

try:
    from .config import RAGConfig
    from .corpus import CorpusIndex
    from .embedders import ImageEmbedder, TextEmbedder
    from .loader import load_directory, load_records
    from .store import Hit, Record, VectorStore
except ImportError:  # 作为脚本直接运行时退化为绝对导入
    from config import RAGConfig
    from corpus import CorpusIndex
    from embedders import ImageEmbedder, TextEmbedder
    from loader import load_directory, load_records
    from store import Hit, Record, VectorStore


class RAGService:
    def __init__(self, cfg: RAGConfig) -> None:
        self.cfg = cfg
        self.store = VectorStore.load(cfg.index_path())
        self.text_embedder = TextEmbedder(cfg)
        self.image_embedder = ImageEmbedder(cfg, self.text_embedder)
        self._corpus: CorpusIndex | None = None
        if cfg.corpus_db and Path(cfg.corpus_db).exists():
            try:
                self._corpus = CorpusIndex(cfg.corpus_db)
            except Exception as e:  # noqa: BLE001 - 索引损坏不应让服务起不来
                print(f"[warn] 典籍索引不可用，已跳过：{e}")

    # ---- 索引构建 / 增量 ----
    async def build_from_corpus(self) -> int:
        if self.cfg.corpus_dir and Path(self.cfg.corpus_dir).is_dir():
            recs = load_directory(Path(self.cfg.corpus_dir))
        else:
            recs = []
        for r in recs:
            await self._embed_record(r)
        for r in recs:
            self.store.add(r)
        self.store.save(self.cfg.index_path())
        return len(self.store.records)

    async def _embed_record(self, r: Record) -> None:
        if r.text and not r.text_vec:
            r.text_vec = await self.text_embedder.embed_one(r.text)
        if r.image_path and not r.image_vec:
            cap, vec = await self.image_embedder.embed_image(r.image_path)
            r.image_caption = r.image_caption or cap
            r.image_vec = vec

    async def ingest(self, payload: list[dict]) -> int:
        recs = load_records(payload)
        for r in recs:
            await self._embed_record(r)
            self.store.add(r)
        self.store.save(self.cfg.index_path())
        return len(recs)

    async def ingest_image(self, image_path: str, caption: str | None = None,
                           text: str | None = None) -> str:
        rid = f"img::{Path(image_path).stem}"
        cap, vec = await self.image_embedder.embed_image(image_path)
        cap = caption or cap
        if text and not vec:
            vec = await self.text_embedder.embed_one(text)
        rec = Record(id=rid, text=text or "", image_path=image_path,
                     image_caption=cap, image_vec=vec,
                     meta={"kind": "paired", "source": image_path})
        if text and not rec.text_vec:
            rec.text_vec = await self.text_embedder.embed_one(text)
        self.store.add(rec)
        self.store.save(self.cfg.index_path())
        return rid

    # ---- 检索 ----
    async def retrieve_text(self, query: str, top_k: int | None = None) -> list[dict]:
        k = top_k or self.cfg.top_k
        vec = await self.text_embedder.embed_one(query)
        hits = self.store.search(vec, modality="text",
                                 top_k=k,
                                 threshold=self.cfg.score_threshold,
                                 query_text=query)
        out = [self._hit_to_dict(h) for h in hits]
        # 典籍语料补充（T4.3）：向量域没召回满时，用离线索引补齐。
        # 两者是互补而非替代——向量擅长语义相近，bigram 索引擅长精确用词。
        if len(out) < k and self._corpus is not None:
            seen = {h["text"][:40] for h in out}
            # 语料检索是**同步且读盘**的，放进线程池执行，避免阻塞事件循环
            corpus_hits = await __import__("asyncio").get_running_loop().run_in_executor(
                None,
                lambda: self._corpus.search(
                    query,
                    top_k=k,
                    top_docs=self.cfg.corpus_top_docs,
                    max_chars=self.cfg.corpus_max_chars,
                    overlap=self.cfg.corpus_overlap,
                ),
            )
            for ch in corpus_hits:
                if ch.text[:40] in seen:
                    continue
                seen.add(ch.text[:40])
                out.append({
                    "id": ch.id,
                    "score": round(ch.score, 4),
                    "modality": "text",
                    "text": ch.text,
                    "image_path": None,
                    "image_caption": None,
                    "meta": {
                        **ch.meta,
                        # 出处带上篇名：模型引用时能说清「出自哪一本的哪一篇」
                        "source": f"《{ch.book}》{ch.meta.get('section', '')}".rstrip(),
                        "kind": "corpus",
                    },
                })
                if len(out) >= k:
                    break
        return out

    def corpus_stats(self) -> dict | None:
        return self._corpus.stats() if self._corpus else None

    async def retrieve_image(self, image_path: str, top_k: int | None = None) -> list[dict]:
        """以图搜图：用查询图 caption 向量检索库内图像向量域。"""
        _, vec = await self.image_embedder.embed_image(image_path)
        hits = self.store.search(vec, modality="image",
                                 top_k=top_k or self.cfg.top_k,
                                 threshold=self.cfg.score_threshold)
        return [self._hit_to_dict(h) for h in hits]

    async def retrieve_paired(self, *, query: str | None = None,
                              image_path: str | None = None,
                              top_k: int | None = None) -> list[dict]:
        """图文联合检索：可传入 query（文）或 image_path（图），跨两域召回。"""
        vec = None
        if query:
            vec = await self.text_embedder.embed_one(query)
        elif image_path:
            _, vec = await self.image_embedder.embed_image(image_path)
        if vec is None:
            return []
        hits = self.store.search(vec, modality="paired",
                                 top_k=top_k or self.cfg.top_k,
                                 threshold=self.cfg.score_threshold,
                                 query_text=query or "")
        return [self._hit_to_dict(h) for h in hits]

    @staticmethod
    def _hit_to_dict(h: Hit) -> dict[str, Any]:
        return {
            "id": h.id,
            "score": round(h.score, 4),
            "modality": h.modality,
            "text": h.text,
            "image_path": h.image_path,
            "image_caption": h.image_caption,
            "meta": h.meta,
        }

    # 便捷别名
    async def search(self, query: str, top_k: int | None = None) -> list[dict]:
        return await self.retrieve_text(query, top_k)
