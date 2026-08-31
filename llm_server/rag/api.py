"""RAG HTTP 服务（FastAPI）：为 llm_server 增加文本/图像/图文 RAG 检索能力。

端点：
  GET  /health
  POST /rag/ingest            { "docs": [ {id, text?, image_path?, image_caption?, meta?} ] }
  POST /rag/ingest_image      { "image_path": "...", "caption"?: "...", "text"?: "..." }
  POST /rag/build             { }   从 RAG_CORPUS_DIR 重建索引
  POST /rag/retrieve/text     { "query": "...", "top_k"?: int, "tags"?: [..], "tag_groups"?: [[..]] }
  POST /rag/retrieve/image    { "image_path": "...", "top_k"?: int }
  POST /rag/retrieve/paired   { "query"?: "...", "image_path"?: "...", "top_k"?: int }
  POST /rag/retrieve/scope    { "query": "...", "genres"/"functions"/"departments"/"schools"?: [..] }
  GET  /rag/stats
  GET  /rag/tags?dim=...

`/rag/retrieve/scope` 是给 sub-agent 用的：每个 agent 有自己的**知识域**
（开方只看方书、切诊只看脉学），用四维分类标签圈定检索范围，
维度之间默认取交集——「方书 AND 儿科」而不是「方书 OR 儿科」。
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


# 请求模型必须定义在**模块层**：本模块带 `from __future__ import annotations`，
# 注解都是字符串，FastAPI 要到函数 globals 里解析类型名；若模型定义在
# `create_app` 内部（局部作用域），解析不到就会把 body 参数识别错，
# 表现为所有 POST 端点一律 422（此前正是如此）。

class IngestReq(BaseModel):
    docs: list[dict]


class IngestImageReq(BaseModel):
    image_path: str
    caption: str | None = None
    text: str | None = None


class TextReq(BaseModel):
    """文本检索请求。

    除了 `query` / `top_k`，还可带**知识域**（四维标签 + 扁平 `tags`），
    供 sub-agent 按自己的检索域过滤。字段全部可选，老调用方不带则行为不变。
    """

    query: str
    top_k: int | None = None
    genres: list[str] = []
    functions: list[str] = []
    departments: list[str] = []
    schools: list[str] = []
    require_all: bool = True
    tags: list[str] = []


class ImageReq(BaseModel):
    image_path: str
    top_k: int | None = None


class PairedReq(BaseModel):
    query: str | None = None
    image_path: str | None = None
    top_k: int | None = None


class ScopeReq(BaseModel):
    """按知识域检索：sub-agent 各自圈定自己该看的书。

    四个字段对应分类的四个维度，**维度之间取交集**（`require_all=false`
    可退回并集）。留空的维度不参与过滤。

    例：开方 agent 在辨证为「儿科」后检索「小儿发热咳嗽」：
    `{"query": "小儿发热咳嗽", "genres": ["方书方剂"], "departments": ["儿科"]}`
    -> 方书 ∩ 儿科，命中《小儿痘疹方论》这类专科方书。
    """

    query: str
    top_k: int | None = None
    genres: list[str] = []
    functions: list[str] = []
    departments: list[str] = []
    schools: list[str] = []
    #: true=跨维度交集（默认）；false=所有维度扁平并集
    require_all: bool = True


def scope_to_tag_groups(req) -> tuple[list[list[str]], list[str]]:
    """把四维 scope 编译成 `tag_groups`（组间交集）或扁平 `tags`（并集）。

    抽成模块级函数是为了可测：这组语义是 sub-agent 检索正确与否的关键，
    不该埋在路由闭包里。

    按鸭子类型取字段（`ScopeReq` 与 `TextReq` 都有这几个字段），
    避免为同一个语义写两遍。
    """
    dims = [getattr(req, "genres", None) or [],
            getattr(req, "functions", None) or [],
            getattr(req, "departments", None) or [],
            getattr(req, "schools", None) or []]
    present = [d for d in dims if d]
    if not present:
        return [], []
    if not getattr(req, "require_all", True):
        return [], [t for d in present for t in d]
    return [list(d) for d in present], []


def create_app(cfg: RAGConfig | None = None) -> FastAPI:
    cfg = cfg or RAGConfig.from_env()
    svc = RAGService(cfg)
    app = FastAPI(title="TCM RAG Service", version="0.1.0")

    @app.get("/health")
    async def health():
        return {"status": "ok", "docs": len(svc.store.records),
                "corpus": svc.corpus_stats()}

    @app.post("/rag/ingest")
    async def ingest(req: IngestReq):
        n = await svc.ingest(req.docs)
        return {"ingested": n, "total": len(svc.store.records)}

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

    @app.post("/rag/retrieve/text")
    async def retrieve_text(req: TextReq):
        # 带知识域时走同一套编译逻辑（四维交集），不带则退化为无过滤
        groups, flat = scope_to_tag_groups(req)
        return await svc.retrieve_text(
            req.query, req.top_k,
            tags=(flat + req.tags) or None,
            tag_groups=groups or None)

    @app.post("/rag/retrieve/scope")
    async def retrieve_scope(req: ScopeReq):
        """按知识域检索典籍语料（sub-agent 专用通道）。"""
        groups, flat = scope_to_tag_groups(req)
        return await svc.retrieve_corpus(
            req.query, req.top_k, tags=flat or None, tag_groups=groups or None)

    @app.get("/rag/tags")
    async def tags(dim: str | None = None):
        """列出可用标签；`dim` 指定维度时只列该维度。"""
        if svc._corpus is None:
            raise HTTPException(503, "corpus index not available")
        return svc._corpus.tag_counts(dim)

    @app.post("/rag/retrieve/image")
    async def retrieve_image(req: ImageReq):
        if not Path(req.image_path).exists():
            raise HTTPException(400, "image_path not found")
        return await svc.retrieve_image(req.image_path, req.top_k)

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
        return {**svc.store.to_dict(), "corpus": svc.corpus_stats()}

    return app
