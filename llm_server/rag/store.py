"""向量存储与检索（纯 Python + numpy；可选 annoy 加速）。

- 每条记录可同时持有：
    * ``text_vec``   : 文本描述经 Embedding 得到的向量（用于文本/图文检索）
    * ``image_vec``  : 图像 caption 经 Embedding 得到的向量（用于图像检索）
- 检索使用余弦相似度；同时提供基于关键字的轻量全文回退，保证无向量时也可用。
"""
from __future__ import annotations

import json
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import numpy as np

try:  # 可选依赖：annoy 提供大规模近似最近邻；缺失则退回暴力检索
    from annoy import AnnoyIndex

    _HAS_ANNOY = True
except Exception:  # noqa: BLE001
    _HAS_ANNOY = False


_TOKEN_RE = re.compile(r"[一-鿿\w]+", re.UNICODE)


@dataclass
class Record:
    id: str
    text: str = ""
    image_path: str | None = None
    image_caption: str | None = None
    meta: dict = field(default_factory=dict)
    text_vec: list[float] | None = None
    image_vec: list[float] | None = None

    def to_dict(self) -> dict:
        # 向量转原生 float，避免 numpy.float32 不可 JSON 序列化
        def _to_floats(v):
            return [float(x) for x in v] if v is not None else None

        return {
            "id": self.id,
            "text": self.text,
            "image_path": self.image_path,
            "image_caption": self.image_caption,
            "meta": self.meta,
            "text_vec": _to_floats(self.text_vec),
            "image_vec": _to_floats(self.image_vec),
        }

    @classmethod
    def from_dict(cls, d: dict) -> "Record":
        return cls(
            id=d["id"], text=d.get("text", ""), image_path=d.get("image_path"),
            image_caption=d.get("image_caption"), meta=d.get("meta") or {},
            text_vec=d.get("text_vec"), image_vec=d.get("image_vec"),
        )


@dataclass
class Hit:
    id: str
    score: float
    text: str = ""
    image_path: str | None = None
    image_caption: str | None = None
    meta: dict = field(default_factory=dict)
    modality: str = "text"   # text | image | paired


def _cosine(a: np.ndarray, b: np.ndarray) -> float:
    na = np.linalg.norm(a)
    nb = np.linalg.norm(b)
    if na == 0.0 or nb == 0.0:
        return 0.0
    return float(np.dot(a, b) / (na * nb))


def _keyword_score(query: str, text: str) -> float:
    # 中文无空格分词，使用字符集合重叠比作为回退打分（无需分词器）。
    q = set(query.lower())
    t = set((text or "").lower())
    q = {c for c in q if c.strip()}
    if not q:
        return 0.0
    overlap = len(q & t)
    return overlap / len(q)


class VectorStore:
    def __init__(self, records: list[Record] | None = None) -> None:
        self.records: list[Record] = records or []
        self._by_id: dict[str, Record] = {r.id: r for r in self.records}

    # ---- 写入 ----
    def add(self, rec: Record) -> None:
        if rec.id in self._by_id:
            # 合并：保留非空字段
            old = self._by_id[rec.id]
            for f in ("text", "image_path", "image_caption", "text_vec", "image_vec"):
                v = getattr(rec, f)
                if v is None:
                    setattr(rec, f, getattr(old, f))
            rec.meta = {**old.meta, **rec.meta}
        self._by_id[rec.id] = rec
        if rec not in self.records:
            self.records.append(rec)

    def upsert(self, rec: Record) -> None:
        self.add(rec)

    # ---- 检索 ----
    def _vecs(self, field_name: str) -> tuple[list[Record], np.ndarray | None]:
        rs = [r for r in self.records if getattr(r, field_name) is not None]
        if not rs:
            return rs, None
        mat = np.array([getattr(r, field_name) for r in rs], dtype=np.float32)
        return rs, mat

    def search(self, query_vec: list[float] | None, *, modality: str,
               top_k: int = 5, threshold: float = 0.0,
               query_text: str = "") -> list[Hit]:
        """按 modality 在对应向量域检索；无向量时退化为关键字匹配。

        modality: "text" -> text_vec；"image" -> image_vec；"paired" -> 两者并集。
        """
        qv = np.array(query_vec, dtype=np.float32) if query_vec else None
        hits: list[Hit] = []

        if modality in ("text", "paired") and qv is not None:
            rs, mat = self._vecs("text_vec")
            if mat is not None:
                sims = mat @ qv / (np.linalg.norm(mat, axis=1) * np.linalg.norm(qv) + 1e-9)
                for r, s in zip(rs, sims):
                    if s >= threshold:
                        hits.append(Hit(r.id, float(s), r.text, r.image_path,
                                        r.image_caption, r.meta, "text"))

        if modality in ("image", "paired") and qv is not None:
            rs, mat = self._vecs("image_vec")
            if mat is not None:
                sims = mat @ qv / (np.linalg.norm(mat, axis=1) * np.linalg.norm(qv) + 1e-9)
                for r, s in zip(rs, sims):
                    if s >= threshold:
                        hits.append(Hit(r.id, float(s), r.text, r.image_path,
                                        r.image_caption, r.meta, "image"))

        if not hits and query_text:
            # 全文回退：对同时含文本与图像描述的记录做 token 重叠打分
            for r in self.records:
                blob = " ".join(x for x in (r.text, r.image_caption) if x)
                sc = _keyword_score(query_text, blob)
                if sc > 0:
                    hits.append(Hit(r.id, sc, r.text, r.image_path,
                                    r.image_caption, r.meta, "text"))

        hits.sort(key=lambda h: h.score, reverse=True)
        return hits[:top_k]

    # ---- 持久化 ----
    def save(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        payload = {
            "version": 1,
            "records": [r.to_dict() for r in self.records],
        }
        path.write_text(json.dumps(payload, ensure_ascii=False, indent=2),
                        encoding="utf-8")

    @classmethod
    def load(cls, path: Path) -> "VectorStore":
        if not path.exists():
            return cls([])
        data = json.loads(path.read_text(encoding="utf-8"))
        recs = [Record.from_dict(d) for d in data.get("records", [])]
        return cls(recs)

    def to_dict(self) -> dict[str, Any]:
        return {"count": len(self.records), "has_annoy": _HAS_ANNOY}
