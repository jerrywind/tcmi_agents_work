"""把 RAG（中医典籍检索）挂进 llm_server 主应用。

为什么要这一层：
harness 的 `rag_endpoint` 指向的是 **llm_server** 而不是独立 RAG 服务，
但主应用此前只挂了 gateway 路由，RAG 端点是 404——整条链路因此在空转
（技能 `tcm-rag` 调用时拿不到东西）。这里把 RAG 的路由并进主应用。

为什么是「可选」而不是硬性依赖：
RAG 依赖典籍索引（SQLite）与 embedding 端点，二者在部署环境里都可能缺失。
RAG 是**增强能力**而非网关的组成部分——索引没建好时网关必须照常工作，
故初始化失败只降级，不阻断启动。
"""
from __future__ import annotations

import logging
import sys
from pathlib import Path

from fastapi import APIRouter, HTTPException
from fastapi.responses import JSONResponse

logger = logging.getLogger("llm_server")

# `rag` 是 llm_server 下的同级包，但 llm_server 未必以包的形式被导入
# （uvicorn app.main:app 时工作目录是 llm_server，rag 不在包路径下）。
# 与 rag/__main__.py 一样把 llm_server 目录加进 sys.path，保证两种启动方式都可用。
_LLM_SERVER_DIR = str(Path(__file__).resolve().parent.parent)
if _LLM_SERVER_DIR not in sys.path:
    sys.path.insert(0, _LLM_SERVER_DIR)


def _disabled_router(reason: str) -> APIRouter:
    """RAG 不可用时的占位路由。

    保留 `/rag/health` 让调用方能**明确区分**「RAG 没配好」和「地址写错了」——
    返回 503 + 原因，而不是让整个 /rag/* 静默 404。
    """
    router = APIRouter()

    @router.get("/rag/health")
    async def rag_health():
        return JSONResponse(
            status_code=503,
            content={"status": "unavailable", "reason": reason},
        )

    @router.post("/rag/retrieve/{rest:path}")
    @router.get("/rag/{rest:path}")
    async def rag_unavailable(rest: str):
        raise HTTPException(503, f"RAG 不可用：{reason}")

    return router


def build_rag_router() -> APIRouter:
    """构造 RAG 路由；不可用时返回降级占位。"""
    try:
        from rag.api import create_app
        from rag.config import RAGConfig
    except ImportError as exc:
        logger.warning("RAG 未挂载（导入失败：%s）", exc)
        return _disabled_router(f"导入失败：{exc}")

    try:
        cfg = RAGConfig.from_env()
        rag_app = create_app(cfg)
    except Exception as exc:  # noqa: BLE001 - 索引损坏不应让网关起不来
        logger.warning("RAG 未挂载（初始化失败：%s）", exc)
        return _disabled_router(f"初始化失败：{exc}")

    logger.info(
        "RAG 已挂载：corpus_db=%s top_k=%s",
        cfg.corpus_db or "(未配置)", cfg.top_k,
    )
    # FastAPI 实例内部就是一个 APIRouter，直接并过来即可保留 /rag/* 路径
    return rag_app.router
