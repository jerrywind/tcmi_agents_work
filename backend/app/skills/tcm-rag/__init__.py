"""tcm-rag 技能：对接 llm_server 中的 RAG 检索服务，为多个中医子智能体提供知识检索能力。

提供三类检索（与 llm_server/rag 的检索器一一对应）：
- rag_text_retrieve  ：文本 -> 文本向量域最近邻（以文搜文），辨证 / 施治 / 问诊均可调用。
- rag_image_retrieve ：图像 -> 图像向量域最近邻（以图搜图），望诊 / 辨证可用。
- rag_paired_retrieve：图文联合检索（以文搜图 / 以图搜文），辨证 / 施治 / 望诊可用。

当 RAG 服务不可用（未部署 / 离线）时，工具优雅降级：返回 ``ok=false`` 与空结果，
绝不抛异常中断 agent 推理。是否启用由 ``TCM_RAG_BASE_URL`` 环境变量控制。
"""
from __future__ import annotations

import os
from typing import Any

import httpx

from app.skills.types import SkillManifest, ToolSpec

RAG_BASE_URL = os.environ.get("TCM_RAG_BASE_URL", "http://llm_server:8080").rstrip("/")
RAG_API_KEY = os.environ.get("TCM_RAG_API_KEY", "")
_TIMEOUT = float(os.environ.get("TCM_RAG_TIMEOUT", "8"))


def _headers() -> dict[str, str]:
    h = {"Content-Type": "application/json"}
    if RAG_API_KEY:
        h["Authorization"] = f"Bearer {RAG_API_KEY}"
    return h


async def _post(path: str, payload: dict) -> dict[str, Any]:
    """调用 RAG 服务；任何失败都降级为 ok=false。"""
    url = f"{RAG_BASE_URL}{path}"
    try:
        async with httpx.AsyncClient(timeout=_TIMEOUT) as client:
            resp = await client.post(url, json=payload, headers=_headers())
            resp.raise_for_status()
            data = resp.json()
        raw = data if isinstance(data, list) else data.get("results", [])
        # 规范化：把每条结果的出处（书籍/章节）提升到顶层 source_book，便于引用标注
        results = [_normalize(rec) for rec in raw]
        return {"ok": True, "results": results, "source": "rag"}
    except Exception as exc:  # noqa: BLE001 - 检索失败不应中断诊疗流程
        return {"ok": False, "results": [], "reason": f"rag_unavailable: {exc}"}


def _normalize(rec: dict) -> dict:
    """归一化 RAG 结果：提取 meta.source_book / meta.source 作为顶层 source_book 字段。"""
    rec = dict(rec)
    meta = rec.get("meta") or {}
    src = rec.get("source_book") or meta.get("source_book") or meta.get("source") or ""
    if src:
        rec["source_book"] = src
    return rec


async def rag_text_retrieve(query: str, top_k: int = 5) -> dict:
    """依据自然语言查询，从文本知识库中检索最相关的条目（以文搜文）。

    适用于：辨证（核对证候依据）、施治（查找治法/方剂出处）、问诊（检索相似主诉）。
    """
    return await _post("/rag/retrieve/text", {"query": query, "top_k": int(top_k)})


async def rag_image_retrieve(image_path: str, top_k: int = 5) -> dict:
    """依据一张图片，从图像知识库中检索相似图像/病例（以图搜图）。

    适用于：望诊（检索与当前舌象/面象/患处相似的既往图像）、辨证（图像证据比对）。
    """
    return await _post("/rag/retrieve/image", {"image_path": image_path, "top_k": int(top_k)})


async def rag_paired_retrieve(query: str | None = None, image_path: str | None = None,
                              top_k: int = 5) -> dict:
    """图文联合检索：可传入 query（文）或 image_path（图），跨文本/图像两域召回。

    适用于：以文搜图 / 以图搜文，常用于辨证与施治阶段的多模态证据补全。
    """
    payload: dict[str, Any] = {"top_k": int(top_k)}
    if query:
        payload["query"] = query
    if image_path:
        payload["image_path"] = image_path
    return await _post("/rag/retrieve/paired", payload)


SKILL = SkillManifest(
    name="tcm-rag",
    version="0.1.0",
    description="对接 RAG 检索服务的多模态知识检索技能（文本/图像/图文联合），"
                "为辨证、施治、问诊、望诊等子智能体提供可检索的中医知识库支撑。",
    tools=[
        ToolSpec(
            name="rag_text_retrieve",
            description="文本检索：输入一段自然语言查询，返回最相关的文本知识条目（以文搜文）。"
                        "用于核对证候依据、查找治法/方剂出处、检索相似主诉。"
                        "每条结果带 source_book 出处字段（如《伤寒论》），引用时务必标注出处。",
            parameters={
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "检索查询文本"},
                    "top_k": {"type": "integer", "description": "返回条数，默认 5", "default": 5},
                },
                "required": ["query"],
            },
            capability=["diagnosis.differentiation", "treatment.plan", "diagnosis.inquiry"],
        ),
        ToolSpec(
            name="rag_image_retrieve",
            description="图像检索：输入一张图片路径，返回相似图像/病例（以图搜图）。"
                        "用于望诊相似舌象/面象比对、辨证图像证据补全。",
            parameters={
                "type": "object",
                "properties": {
                    "image_path": {"type": "string", "description": "待检索图片路径"},
                    "top_k": {"type": "integer", "description": "返回条数，默认 5", "default": 5},
                },
                "required": ["image_path"],
            },
            capability=["diagnosis.inspection", "diagnosis.differentiation"],
        ),
        ToolSpec(
            name="rag_paired_retrieve",
            description="图文联合检索：可传入 query（文）或 image_path（图），跨文本/图像两域召回"
                        "相关条目，返回结果带 modality 标记（以文搜图 / 以图搜文）。用于多模态证据补全。",
            parameters={
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "（可选）文本查询"},
                    "image_path": {"type": "string", "description": "（可选）图片路径"},
                    "top_k": {"type": "integer", "description": "返回条数，默认 5", "default": 5},
                },
                "required": [],
            },
            capability=["diagnosis.differentiation", "treatment.plan", "diagnosis.inspection"],
        ),
    ],
)

HANDLERS = {
    "rag_text_retrieve": rag_text_retrieve,
    "rag_image_retrieve": rag_image_retrieve,
    "rag_paired_retrieve": rag_paired_retrieve,
}
