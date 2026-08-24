"""语料加载：从目录构造 Record（支持图文配对与纯文本两种来源）。

目录约定（RAG_CORPUS_DIR 指向的根目录）：
- images/<id>.jpg  +  images/<id>.txt  -> 图文配对（txt 为该图的文本描述/标签）
- texts/<name>.txt|.md -> 纯文本片段（按空行或标题切分）

也支持直接以 dict 列表 ingest（见 api.retriever.ingest）。
"""
from __future__ import annotations

import re
from pathlib import Path

try:
    from .store import Record
except ImportError:  # 作为脚本直接运行时退化为绝对导入
    from store import Record

_SPLIT_RE = re.compile(r"\n\s*\n")


def load_directory(corpus_dir: Path) -> list[Record]:
    recs: list[Record] = []

    # 1) 纯文本
    texts_dir = corpus_dir / "texts"
    if texts_dir.is_dir():
        for p in sorted(texts_dir.rglob("*")):
            if p.suffix.lower() not in (".txt", ".md"):
                continue
            raw = p.read_text(encoding="utf-8", errors="ignore")
            for i, chunk in enumerate(_SPLIT_RE.split(raw)):
                chunk = chunk.strip()
                if len(chunk) < 4:
                    continue
                rid = f"txt::{p.stem}:{i}"
                recs.append(Record(id=rid, text=chunk,
                                   meta={"source": str(p), "kind": "text"}))

    # 2) 图文配对：images/<id>.{jpg,png} 与同名 .txt
    images_dir = corpus_dir / "images"
    if images_dir.is_dir():
        for img in sorted(images_dir.iterdir()):
            if img.suffix.lower() not in (".jpg", ".jpeg", ".png", ".bmp", ".webp"):
                continue
            txt = images_dir / f"{img.stem}.txt"
            caption_text = txt.read_text(encoding="utf-8", errors="ignore").strip() if txt.exists() else ""
            rid = f"img::{img.stem}"
            recs.append(Record(
                id=rid,
                text=caption_text,
                image_path=str(img),
                image_caption=caption_text or None,
                meta={"source": str(img), "kind": "paired"},
            ))
    return recs


def load_records(payload: list[dict]) -> list[Record]:
    """从 API ingest 传入的 dict 列表构造 Record。"""
    out: list[Record] = []
    for d in payload:
        out.append(Record(
            id=d["id"],
            text=d.get("text", ""),
            image_path=d.get("image_path"),
            image_caption=d.get("image_caption"),
            meta=d.get("meta") or {},
        ))
    return out
