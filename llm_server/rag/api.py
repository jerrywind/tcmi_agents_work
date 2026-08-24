"""RAG HTTP 服务（FastAPI）：为 llm_server 增加文本/图像/图文 RAG 检索能力。

端点：
  GET  /health
  POST /rag/ingest            { "docs": [ {id, text?, image_path?, image_caption?, meta?} ] }
  POST /rag/ingest_image      { "image_path": "...", "caption"?: "...", "text"?: "..." }
  POST /rag/build             { }   从 RAG_CORPUS_DIR 重建索引
  POST /rag/retrieve/text     { "query": "...", "top_k"?: int }
  POST /rag/retrieve/image    { "image_path": "...", "top_k"?: int }
  POST /rag/retrieve/paired   { "query"?: "...", "image_path"?: "...", "top_k"?: int }
  GET  /rag/stats
"""
from __future__ import annotations

from pathlib import Path

from fastapi import FastAPI, HTTPException
from pydantic import BaseModel

try:
    from .config import RAGConfig
    from .retriever import RAGService
except ImportError:  # 作为脚本直接运行时退化为绝对导入
    from config import RAGConfig
    from retriever import RAGService


def create_app(cfg: RAGConfig | None = None) -> FastAPI:
    cfg = cfg or RAGConfig.from_env()
    svc = RAGService(cfg)
    app = FastAPI(title="TCM RAG Service", version="0.1.0")

    @app.get("/health")
    async def health():
        return {"status": "ok", "docs": len(svc.store.records)}

    class IngestReq(BaseModel):
        docs: list[dict]

    @app.post("/rag/ingest")
    async def ingest(req: IngestReq):
        n = await svc.ingest(req.docs)
        return {"ingested": n, "total": len(svc.store.records)}

    class IngestImageReq(BaseModel):
        image_path: str
        caption: str | None = None
        text: str | None = None

    @app.post("/rag/ingest_image")
    async def ingest_image(req: IngestImageReq):
        if not Path(req.image_path).exists():
            raise HTTPException(400, "image_path not found")
        rid = await svc.ingest_image(req.image_path, req.caption, req.text)
        return {"id": rid, "total": len(svc.store.records)}

    @app.post("/rag/build")
    async def build():
        n = await svc.build_from_corpus()
        return {"built": n}

    class TextReq(BaseModel):
        query: str
        top_k: int | None = None

    @app.post("/rag/retrieve/text")
    async def retrieve_text(req: TextReq):
        return await svc.retrieve_text(req.query, req.top_k)

    class ImageReq(BaseModel):
        image_path: str
        top_k: int | None = None

    @app.post("/rag/retrieve/image")
    async def retrieve_image(req: ImageReq):
        if not Path(req.image_path).exists():
            raise HTTPException(400, "image_path not found")
        return await svc.retrieve_image(req.image_path, req.top_k)

    class PairedReq(BaseModel):
        query: str | None = None
        image_path: str | None = None
        top_k: int | None = None

    @app.post("/rag/retrieve/paired")
    async def retrieve_paired(req: PairedReq):
        if not req.query and not req.image_path:
            raise HTTPException(400, "provide query or image_path")
        if req.image_path and not Path(req.image_path).exists():
            raise HTTPException(400, "image_path not found")
        return await svc.retrieve_paired(query=req.query, image_path=req.image_path,
                                         top_k=req.top_k)

    @app.get("/rag/stats")
    async def stats():
        return svc.store.to_dict()

    return app
