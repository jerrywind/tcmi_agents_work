"""检索器：封装三种 RAG 模式。

- text   ：文本 -> Embedding -> 文本向量域最近邻
- image  ：图像 -> Qwen3-VL caption -> Embedding -> 图像向量域最近邻
- paired ：图文联合——可用文本或图像查询，跨 text_vec / image_vec 两个域召回，
           并按查询模态与文档模态给出 modality 标记（以文搜图 / 以图搜文）。
"""
from __future__ import annotations

from pathlib import Path
from typing import Any, Sequence

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
    def _corpus_search(self, query: str, k: int, *,
                       tags: Sequence[str] | None = None,
                       tag_groups: Sequence[Sequence[str]] | None = None):
        """同步语料检索（在 `retrieve_corpus` / `retrieve_text` 里丢进线程池）。

        `_corpus` 为 None（索引未构建/损坏）时返回空列表，而不是抛异常——
        典籍检索是可降级能力，不该让整个 RAG 请求失败。
        """
        if self._corpus is None:
            return []
        return self._corpus.search(
            query,
            top_k=k,
            top_docs=self.cfg.corpus_top_docs,
            max_chars=self.cfg.corpus_max_chars,
            overlap=self.cfg.corpus_overlap,
            tags=tags,
            tag_groups=tag_groups,
        )

    async def _corpus_search_async(self, query: str, k: int, **kw):
        # 语料检索是**同步且读盘**的，放进线程池执行，避免阻塞事件循环
        loop = __import__("asyncio").get_running_loop()
        return await loop.run_in_executor(
            None, lambda: self._corpus_search(query, k, **kw))

    async def retrieve_corpus(self, query: str, top_k: int | None = None, *,
                              tags: Sequence[str] | None = None,
                              tag_groups: Sequence[Sequence[str]] | None = None
                              ) -> list[dict]:
        """只查典籍语料（离线 bigram 索引），**带知识域过滤**。

        与 `retrieve_text` 里的语料补齐不同，这是**一等通道**而非兜底：
        sub-agent 各有自己的检索域（开方只看方书、切诊只看脉学），
        必须能主动、独立地检索，而不是等向量域召不满才轮到它。

        `tag_groups` 做跨维度交集，如「体裁=方书方剂 AND 科室=儿科」，
        语义见 `CorpusIndex.doc_ords_for_tags`。
        """
        k = top_k or self.cfg.top_k
        return [self._chunk_to_dict(ch)
                for ch in await self._corpus_search_async(
                    query, k, tags=tags, tag_groups=tag_groups)]

    async def retrieve_text(self, query: str, top_k: int | None = None, *,
                            tags: Sequence[str] | None = None,
                            tag_groups: Sequence[Sequence[str]] | None = None
                            ) -> list[dict]:
        k = top_k or self.cfg.top_k
        vec = await self.text_embedder.embed_one(query)
        hits = self.store.search(vec, modality="text",
                                 top_k=k,
                                 threshold=self.cfg.score_threshold,
                                 query_text=query)
        out = [self._hit_to_dict(h) for h in hits]
        # 典籍语料补充（T4.3）：向量域没召回满时，用离线索引补齐。
        # 两者是互补而非替代——向量擅长语义相近，bigram 索引擅长精确用词。
        if len(out) < k:
            seen = {h["text"][:40] for h in out}
            corpus_hits = await self._corpus_search_async(
                query, k, tags=tags, tag_groups=tag_groups)
            for ch in corpus_hits:
                if ch.text[:40] in seen:
                    continue
                seen.add(ch.text[:40])
                out.append(self._chunk_to_dict(ch))
                if len(out) >= k:
                    break
        return out

    @staticmethod
    def _chunk_to_dict(ch) -> dict:
        """语料片段 -> 与向量命中同构的字典，方便调用方无差别处理。"""
        return {
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
        }

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
