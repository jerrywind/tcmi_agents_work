"""RAG 服务命令行入口。

用法：
  python -m rag serve                 # 启动 HTTP 服务（端口见 RAG_PORT，默认 8080）
  python -m rag build                 # 从 RAG_CORPUS_DIR 重建向量索引
  python -m rag ingest --image p.jpg --caption "..." --text "..."
  python -m rag corpus-build  --dir ../rag_data     # 建典籍语料索引（T4.3）
  python -m rag corpus-search --query "半夏泻心汤"
  python -m rag corpus-stats
  python -m rag eval --queries eval/tcm_queries.jsonl   # 召回质量评估（T4.3）

依赖：fastapi + uvicorn + httpx + numpy（annoy 可选）。
典籍语料索引只依赖标准库（sqlite3），可离线使用。
"""
from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

# 允许以脚本方式直接运行（python rag/__main__.py）
sys.path.insert(0, str(Path(__file__).resolve().parent.parent.parent))

from .config import RAGConfig                       # noqa: E402
from .retriever import RAGService                  # noqa: E402


def _serve() -> None:
    import uvicorn

    cfg = RAGConfig.from_env()
    port = int(os.environ.get("RAG_PORT", "9000"))
    app = __import__("fastapi").FastAPI()  # placeholder; replaced below
    from .api import create_app

    app = create_app(cfg)
    uvicorn.run(app, host="0.0.0.0", port=port)


def _build() -> None:
    cfg = RAGConfig.from_env()
    svc = RAGService(cfg)
    n = __import__("asyncio").run(svc.build_from_corpus())
    print(f"built index with {n} records -> {cfg.index_path()}")


def _ingest_image(args) -> None:
    cfg = RAGConfig.from_env()
    svc = RAGService(cfg)
    rid = __import__("asyncio").run(
        svc.ingest_image(args.image, args.caption, args.text))
    print(f"ingested {rid}")


def _corpus_build(args) -> None:
    """建典籍语料索引（T4.3）。"""
    from .corpus import build_index

    cfg = RAGConfig.from_env()
    src = Path(args.dir) if args.dir else (cfg.corpus_dir or Path("../rag_data"))
    db = Path(args.db) if args.db else cfg.corpus_db
    print(f"索引语料：{src} -> {db}（694 部典籍约需数分钟，请耐心）", flush=True)
    stat = build_index(src, db, progress=True, limit=args.limit)
    print(f"完成：{stat['books']} 部 / {stat['chars']:,} 字 -> {stat['path']}")


def _corpus_search(args) -> None:
    from .corpus import search_index

    cfg = RAGConfig.from_env()
    db = Path(args.db) if args.db else cfg.corpus_db
    hits = search_index(db, args.query, top_k=args.top_k,
                        top_docs=args.top_docs)
    if not hits:
        print("（无命中：索引未构建或查询词在索引中不存在）")
        return
    for i, h in enumerate(hits, 1):
        print(f"\n[{i}] {h['score']}  《{h['book']}》片段#{h['meta']['chunk']}")
        print(h["text"][:300])


def _corpus_stats(args) -> None:
    from .corpus import CorpusIndex

    cfg = RAGConfig.from_env()
    db = Path(args.db) if args.db else cfg.corpus_db
    if not Path(db).exists():
        print(f"索引不存在：{db}（先跑 `python -m rag corpus-build`）")
        return
    idx = CorpusIndex(db)
    s = idx.stats()
    print(f"典籍 {s['books']} 部 / 汉字 {s['chars']:,} / term {s['terms']:,} / posting {s['postings']:,}")
    for b in idx.books(limit=5):
        print(f"  #{b['ord']:>3} {b['title']}（{b['chars']:,} 字）")
    idx.close()


def _eval(args) -> None:
    """召回质量评估（T4.3）。"""
    from .eval_rag import run_eval

    cfg = RAGConfig.from_env()
    db = Path(args.db) if args.db else cfg.corpus_db
    report = run_eval(
        Path(args.queries),
        db,
        top_k=args.top_k,
        top_docs=args.top_docs,
    )
    print(report["text"])
    if args.out:
        Path(args.out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.out).write_text(
            __import__("json").dumps(report["json"], ensure_ascii=False, indent=2),
            encoding="utf-8",
        )
        print(f"\n报告已写入 {args.out}")


def main(argv: list[str] | None = None) -> None:
    p = argparse.ArgumentParser(prog="rag", description="TCM RAG service")
    sub = p.add_subparsers(dest="cmd", required=True)

    sub.add_parser("serve", help="启动 RAG HTTP 服务")

    sub.add_parser("build", help="从语料目录重建向量索引")

    pi = sub.add_parser("ingest_image", help="增量入库单张图片")
    pi.add_argument("--image", required=True)
    pi.add_argument("--caption", default=None)
    pi.add_argument("--text", default=None)

    pcb = sub.add_parser("corpus-build", help="建典籍语料索引（T4.3，离线可用）")
    pcb.add_argument("--dir", default=None, help="语料目录（默认 $RAG_CORPUS_DIR 或 ../rag_data）")
    pcb.add_argument("--db", default=None, help="索引库路径（默认 $RAG_CORPUS_DB）")
    pcb.add_argument("--limit", type=int, default=None, help="只索引前 N 部（冒烟测试用）")

    pcs = sub.add_parser("corpus-search", help="在典籍语料里检索")
    pcs.add_argument("--query", required=True)
    pcs.add_argument("--top-k", type=int, default=5)
    pcs.add_argument("--top-docs", type=int, default=3)
    pcs.add_argument("--db", default=None)

    pcst = sub.add_parser("corpus-stats", help="查看典籍索引统计")
    pcst.add_argument("--db", default=None)

    pev = sub.add_parser("eval", help="召回质量评估（T4.3）")
    pev.add_argument("--queries", default="eval/tcm_queries.jsonl")
    pev.add_argument("--top-k", type=int, default=5)
    pev.add_argument("--top-docs", type=int, default=3)
    pev.add_argument("--db", default=None)
    pev.add_argument("--out", default=None, help="报告输出路径（JSON）")

    args = p.parse_args(argv)
    if args.cmd == "serve":
        _serve()
    elif args.cmd == "build":
        _build()
    elif args.cmd == "ingest_image":
        _ingest_image(args)
    elif args.cmd == "corpus-build":
        _corpus_build(args)
    elif args.cmd == "corpus-search":
        _corpus_search(args)
    elif args.cmd == "corpus-stats":
        _corpus_stats(args)
    elif args.cmd == "eval":
        _eval(args)


if __name__ == "__main__":
    main()
