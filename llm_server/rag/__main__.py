"""RAG 服务命令行入口。

用法：
  python -m rag serve                 # 启动 HTTP 服务（端口见 RAG_PORT，默认 8080）
  python -m rag build                 # 从 RAG_CORPUS_DIR 重建索引
  python -m rag ingest --image p.jpg --caption "..." --text "..."

依赖：fastapi + uvicorn + httpx + numpy（annoy 可选）。
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


def main(argv: list[str] | None = None) -> None:
    p = argparse.ArgumentParser(prog="rag", description="TCM RAG service")
    sub = p.add_subparsers(dest="cmd", required=True)

    sub.add_parser("serve", help="启动 RAG HTTP 服务")

    sub.add_parser("build", help="从语料目录重建索引")

    pi = sub.add_parser("ingest_image", help="增量入库单张图片")
    pi.add_argument("--image", required=True)
    pi.add_argument("--caption", default=None)
    pi.add_argument("--text", default=None)

    args = p.parse_args(argv)
    if args.cmd == "serve":
        _serve()
    elif args.cmd == "build":
        _build()
    elif args.cmd == "ingest_image":
        _ingest_image(args)


if __name__ == "__main__":
    main()
