"""FastAPI 入口：问诊档案 / 图片上传 / 诊断 loop / 报告 API。"""
from __future__ import annotations

import uuid
import uvicorn
from contextlib import AsyncExitStack, asynccontextmanager
from pathlib import Path

from fastapi import FastAPI, File, Form, HTTPException, UploadFile
from fastapi.middleware.cors import CORSMiddleware
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel

from . import agents  # noqa: F401  触发 sub-agent 注册
from .config import SKILLS_DIR, UPLOAD_DIR, settings
from .mcp import remote_agent  # noqa: F401  触发 impl="mcp" 远程 sub-agent 注册
from .mcp.server import build_http_app, list_tools as mcp_list_tools
from .core.orchestrator import get_task, orchestrator
from .models.schemas import (
    AnswerReq, Consultation, CreateConsultationReq, Evidence, Family, ImageItem,
    Member, PpgReading, RevisitImage, StateResp,
)
from .protocol.base import Capability
from .protocol.registry import available_impls
from .skills.loader import discover_skills, load_skill_by_name, load_skill_from_path
from .skills.registry import skill_registry
from .skills.types import SkillError
from .store import store

# MCP Server（Streamable HTTP）：挂载对象稳定，会话管理器由 lifespan 按次新建
_MCP_APP = build_http_app() if settings.mcp_server_enabled else None


@asynccontextmanager
async def lifespan(_app: FastAPI):
    """应用生命周期：托管 MCP Server 会话管理器与 MCP Client 连接池。"""
    from .mcp.client import tool_hub

    async with AsyncExitStack() as stack:
        if _MCP_APP is not None:
            await stack.enter_async_context(_MCP_APP.run())
        # 按 routing.yaml 的 mcp.clients 自动连接外部 MCP Server（失败不阻塞启动）
        await tool_hub.connect_from_config()
        try:
            yield
        finally:
            await tool_hub.close()


app = FastAPI(title="TCM Consultation Agent", version="0.1.0", lifespan=lifespan)
app.add_middleware(
    CORSMiddleware, allow_origins=settings.cors_origins, allow_credentials=True,
    allow_methods=["*"], allow_headers=["*"],
)
app.mount("/uploads", StaticFiles(directory=str(UPLOAD_DIR)), name="uploads")

# MCP Server：挂载 Streamable HTTP 端点，外部 MCP 客户端可直接接入
if _MCP_APP is not None:
    app.mount(settings.mcp_mount_path, _MCP_APP, name="mcp")

# 启动自动发现并装载 skills/ 目录下的技能（LLM 可调用工具集）
discover_skills(SKILLS_DIR)


def _state(c: Consultation) -> StateResp:
    return StateResp(id=c.id, status=c.status, round=c.round,
                     family_id=c.family_id, member_id=c.member_id,                      ppg=c.ppg,
                     evidences=c.evidences,
                     question=c.current_question, hypotheses=c.hypotheses[:5],
                     messages=c.messages, report=c.report,
                     task_id=c.task_id)


def _get(cid: str) -> Consultation:
    c = store.get(cid)
    if c is None:
        raise HTTPException(404, "consultation not found")
    return c


@app.post("/api/consultations", response_model=StateResp)
async def create_consultation(req: CreateConsultationReq):
    c = Consultation(patient=req.patient, complaint=req.complaint,
                     self_report=req.self_report,
                     family_id=req.family_id, member_id=req.member_id)
    store.save(c)
    return _state(c)


@app.post("/api/consultations/{cid}/images")
async def upload_image(cid: str, type: str = Form(...), file: UploadFile = File(...)):
    c = _get(cid)
    if type not in ("tongue", "face", "lesion", "palm_left", "palm_right"):
        raise HTTPException(400, "type must be tongue|face|lesion|palm_left|palm_right")
    if c.status != "created":
        raise HTTPException(400, "consultation already started")
    suffix = Path(file.filename or "img.jpg").suffix or ".jpg"
    name = f"{cid}_{uuid.uuid4().hex[:8]}{suffix}"
    dest = UPLOAD_DIR / name
    dest.write_bytes(await file.read())
    item = ImageItem(type=type, path=str(dest), url=f"/uploads/{name}")  # type: ignore[arg-type]
    c.images.append(item)
    return {"id": item.id, "url": item.url}


@app.post("/api/consultations/{cid}/start", response_model=StateResp)
async def start(cid: str, sync: bool = False):
    c = _get(cid)
    if c.status == "created":
        if sync:
            # 同步模式：直接跑完整个诊断 Loop 再返回（测试 / 非流式场景用）
            await orchestrator.start_sync(c)
        else:
            await orchestrator.start(c)
    return _state(c)


@app.post("/api/consultations/{cid}/answer", response_model=StateResp)
async def answer(cid: str, req: AnswerReq, sync: bool = False):
    c = _get(cid)
    if c.status not in ("waiting_answer", "treatment_qa") or c.current_question is None:
        raise HTTPException(400, f"not waiting for answer (status={c.status})")
    if req.question_id != c.current_question.id:
        raise HTTPException(409, "question mismatch, refresh state")
    await orchestrator.answer(c, req.value, req.text, sync=sync)
    return _state(c)


@app.get("/api/consultations/{cid}", response_model=StateResp)
async def get_state(cid: str):
    return _state(_get(cid))


@app.get("/api/consultations/{cid}/report")
async def get_report(cid: str):
    c = _get(cid)
    if c.report is None:
        raise HTTPException(404, "report not ready")
    return c.report


@app.get("/api/consultations/{cid}/care")
async def get_care(cid: str):
    """诊疗可执行待办（打卡/煎药提醒）。"""
    return _get(cid).care_todos


class CareCheckReq(BaseModel):
    todo_id: str


@app.post("/api/consultations/{cid}/care/check")
async def check_care(cid: str, req: CareCheckReq):
    """标记某条待办为已完成（今日打卡）。"""
    c = _get(cid)
    for t in c.care_todos:
        if t.id == req.todo_id:
            t.done = True
            return t
    raise HTTPException(404, "todo not found")


@app.get("/api/consultations/{cid}/followups")
async def get_followups(cid: str):
    """随访回访计划。"""
    return _get(cid).followups


class FollowupFeedbackReq(BaseModel):
    feedback: str


@app.post("/api/consultations/{cid}/followup/{fid}/feedback")
async def post_followup_feedback(cid: str, fid: str, req: FollowupFeedbackReq):
    """回访反馈 → 证据回流 + 辨证微调提示。"""
    c = _get(cid)
    await orchestrator.followup_feedback(c, fid, req.feedback)
    return {"ok": True, "followup": next((f for f in c.followups if f.id == fid), None)}


class RevisitReq(BaseModel):
    path: str
    kind: str = "tongue"                       # tongue / face
    self_report: dict = {}


@app.post("/api/consultations/{cid}/revisit")
async def post_revisit(cid: str, req: RevisitReq):
    """复诊上传舌象/面象 → 跑望诊 → 落入 revisits 并与首诊对比。"""
    c = _get(cid)
    resp = await orchestrator._call(
        c, Capability.INSPECTION,
        {"images": [{"type": req.kind, "path": req.path}],
         "self_report": req.self_report, "gender": c.patient.gender})
    features = {e.key: e.value for e in resp.evidences} if resp.status == "ok" else {}
    rv = RevisitImage(path=req.path, kind=req.kind, features=features)
    c.revisits.append(rv)
    return rv


@app.get("/api/consultations/{cid}/revisit/compare")
async def get_revisit_compare(cid: str):
    """首诊 vs 最近一次复诊的望诊特征变化（量化方向）。"""
    c = _get(cid)
    if not c.revisits:
        return {"has_baseline": False, "changes": []}
    base = _baseline_features(c)
    last = c.revisits[-1].features
    changes = []
    for key in sorted(set(base) | set(last)):
        b, a = base.get(key), last.get(key)
        if b != a:
            changes.append({"key": key, "before": b or "—", "after": a or "—",
                            "improved": _trend(key, b, a)})
    return {"has_baseline": bool(base), "baseline_ts": c.ts,
            "revisit_ts": c.revisits[-1].ts, "changes": changes}


class LabReq(BaseModel):
    text: str


@app.post("/api/consultations/{cid}/lab")
async def post_lab(cid: str, req: LabReq):
    """中西医结合：上传西医检验报告文本 → 指标异常提取 + 证候倾向交叉解读 → 证据回流。"""
    c = _get(cid)
    result = await orchestrator.lab_interpret(c, req.text)
    return result


def _baseline_features(c: Consultation) -> dict:
    """首诊望诊特征：来自证据池（source=望）或首次上传图片。"""
    feats = {e.key: e.value for e in c.evidences if e.source == "望"}
    if feats:
        return feats
    # 退化：若首诊也走了 revisit 通道，取最早一条
    return c.revisits[0].features if c.revisits else {}


_ORDER = {
    "tongue.body": ["淡白", "淡红", "红", "红绛"],
    "tongue.coat": ["白腻", "薄白", "薄黄", "黄腻", "少苔"],
    "face.color": ["面色晦暗或浮肿", "萎黄少华", "正常", "颧红", "面赤"],
}


def _trend(key: str, before: str | None, after: str | None) -> str:
    """简单量化：特征值是否朝'更正常'方向移动。"""
    order = _ORDER.get(key)
    if not order or before not in order or after not in order:
        return "unknown"
    normal_idx = order.index("正常") if "正常" in order else None
    b, a = order.index(before), order.index(after)
    if normal_idx is None:
        return "unknown"
    # 越接近 normal 越好
    return "better" if abs(a - normal_idx) < abs(b - normal_idx) else "worse"


@app.get("/api/consultations/{cid}/trace")
async def get_trace(cid: str):
    """sub-agent 调用轨迹（观测每轮由哪个 impl/model 处理）。"""
    return _get(cid).trace


@app.get("/api/consultations/{cid}/stream")
async def get_stream(cid: str, after: int = 0):
    """实时流式分段增量拉取：前端用 after={已读 seq} 轮询，拿到 AI 边说边写的内容。

    返回：{ task: running|done|error, error, segs: [...] }
    - task 取自最近一次 start/answer 的后台任务状态；
    - segs 为 seq>after 的流式分段（含未完成 done=False 的逐段）。
    """
    c = _get(cid)
    task_id = c.meta.get("task_id")
    task = get_task(task_id) if task_id else None
    segs = [s for s in c.stream if s.seq > after]
    return {"task": task["status"] if task else "done",
            "error": task["error"] if task else None,
            "segs": [s.model_dump() for s in segs]}


@app.get("/api/system/agents")
async def list_agents():
    """查看各 capability 当前路由与可用实现，便于运维切换。"""
    out = []
    for cap in Capability:
        route = settings.route_of(cap.value)
        out.append({"capability": cap.value, "current_impl": route["impl"],
                    "model": route["model"], "available_impls": available_impls(cap)})
    return out


# ---------- 家庭 / 成员（一人管理全家档案，多租户准备） ----------
class FamilyCreateReq(BaseModel):
    name: str = "我的家庭"
    owner: str = ""


@app.post("/api/families", response_model=Family)
async def create_family(req: FamilyCreateReq):
    """创建家庭，自动包含一名「本人」成员。"""
    owner = Member(name="本人", relation="本人")
    f = Family(name=req.name, owner=req.owner, members=[owner])
    store.save_family(f)
    return f


@app.get("/api/families", response_model=list[Family])
async def list_families():
    return store.list_families()


def _get_family(fid: str) -> Family:
    f = store.get_family(fid)
    if f is None:
        raise HTTPException(404, "family not found")
    return f


@app.get("/api/families/{fid}", response_model=Family)
async def get_family(fid: str):
    return _get_family(fid)


class MemberAddReq(BaseModel):
    name: str
    relation: str = "其他"
    patient: dict = {}
    note: str = ""


@app.post("/api/families/{fid}/members", response_model=Member)
async def add_member(fid: str, req: MemberAddReq):
    """向家庭添加成员。"""
    f = _get_family(fid)
    from .models.schemas import Patient
    member = Member(name=req.name, relation=req.relation,
                    patient=Patient(**req.patient), note=req.note,
                    family_id=fid)
    f.members.append(member)
    store.save_family(f)
    return member


@app.patch("/api/families/{fid}/members/{mid}", response_model=Member)
async def update_member(fid: str, mid: str, req: MemberAddReq):
    """更新成员档案（体质/备注）。"""
    f = _get_family(fid)
    from .models.schemas import Patient
    for m in f.members:
        if m.id == mid:
            m.name = req.name
            m.relation = req.relation
            m.patient = Patient(**req.patient)
            m.note = req.note
            store.save_family(f)
            return m
    raise HTTPException(404, "member not found")


@app.get("/api/families/{fid}/consultations")
async def family_consultations(fid: str, member_id: str = ""):
    """按家庭（可选成员）列出全部问诊档案（含状态摘要）。"""
    _get_family(fid)
    items = store.list_consultations(family_id=fid, member_id=member_id)
    return [
        {"id": c.id, "member_id": c.member_id, "status": c.status,
         "complaint": c.complaint, "ts": c.ts,
         "syndromes": [h.name for h in (c.report.syndromes if c.report else [])]}
        for c in items
    ]


# ---------- PPG 脉象（硬件接入 / 模拟信号） ----------
class PpgReq(BaseModel):
    samples: list[float] = []                  # 真实硬件采样序列（归一化）
    fs: int = 50                               # 采样率 Hz
    simulate: bool = False                     # 触发内置模拟信号（无硬件演示）
    profile: str = "normal"                    # 模拟波形：normal/slippery/choppy/weak/taut
    rate_bpm: float = 75.0                     # 模拟脉率


async def _handle_ppg(c: Consultation, req: PpgReq) -> dict:
    """PPG 解析核心：合成/解析信号 -> 写入会话 ppg -> 汇入切诊证据池。

    供 REST 端点与 MCP 工具复用。
    """
    from .knowledge.ppg import analyze_ppg, synthesize_ppg, to_evidences
    samples = req.samples
    if req.simulate or not samples:
        samples = synthesize_ppg(fs=req.fs, rate_bpm=req.rate_bpm,
                                 profile=req.profile, seed=abs(hash(c.id)) % 100000)
    res = analyze_ppg(samples, fs=req.fs)

    reading = PpgReading(
        rate_bpm=res.rate_bpm, rhythm=res.rhythm, depth=res.depth, force=res.force,
        shape=res.shape, amplitude=res.amplitude, perfusion=res.perfusion,
        signal_quality=res.signal_quality, notes=res.notes,
    )
    c.ppg = reading
    # 汇入证据池
    c.evidences = [e for e in c.evidences if e.source != "切" or not e.key.startswith("pulse.")]
    for e in to_evidences(res):
        c.evidences.append(Evidence(key=e["key"], value=e["value"],
                                    source="切", confidence=e["confidence"], round=c.round))
    store.save(c)
    return _state(c).model_dump()


@app.post("/api/consultations/{cid}/ppg", response_model=StateResp)
async def upload_ppg(cid: str, req: PpgReq):
    """上传 PPG 采样序列（或触发模拟）并解析为脉象证据。

    解析结果存入会话 ppg 字段，并以高置信度汇入证据池（source=切）。
    之后发起/继续问诊时，切诊 Sub-Agent 会直接采用该脉象。
    """
    c = _get(cid)
    return await _handle_ppg(c, req)


@app.get("/api/health")
async def health():
    return {"ok": True}


# ---------- SKILL 管理（LLM 可调用工具集，支持热装载/卸载） ----------
class SkillLoadReq(BaseModel):
    name: str | None = None   # 按名称装载 skills/ 下的技能
    path: str | None = None   # 或按文件路径装载（目录或 .py）


class SkillUnloadReq(BaseModel):
    name: str


@app.get("/api/skills")
async def list_skills():
    """列出当前已装载的技能及其工具。"""
    return {
        "skills_dir": str(SKILLS_DIR),
        "skills": [s.model_dump() for s in skill_registry.list_skills()],
        "tools": [t.model_dump() for t in skill_registry.list_tools()],
    }


@app.post("/api/skills/load")
async def load_skill(req: SkillLoadReq):
    """运行时热装载技能：提供 name 或 path 之一。"""
    try:
        if req.name:
            manifest = load_skill_by_name(req.name, SKILLS_DIR)
        elif req.path:
            manifest = load_skill_from_path(req.path)
        else:
            raise SkillError("请提供 name 或 path")
    except SkillError as e:
        raise HTTPException(400, str(e))
    return manifest.model_dump()


@app.post("/api/skills/unload")
async def unload_skill(req: SkillUnloadReq):
    """运行时卸载技能（移除其全部工具）。"""
    if not skill_registry.unload(req.name):
        raise HTTPException(404, f"技能 '{req.name}' 未装载")
    return {"ok": True, "unloaded": req.name}


# ---------- MCP 管理（Server 状态 / Client 连接池） ----------
class McpConnectReq(BaseModel):
    name: str
    transport: str = "http"          # http | sse | stdio
    url: str | None = None           # http/sse
    command: str | None = None       # stdio
    args: list[str] = []
    env: dict[str, str] = {}


@app.get("/api/mcp/status")
async def mcp_status():
    """MCP 总览：Server 挂载状态 + 已连接的外部 Server 及其工具数。"""
    from .mcp.client import tool_hub
    from .mcp.tools.agents import capabilities_overview

    return {
        "server": {
            "enabled": settings.mcp_server_enabled,
            "mount_path": settings.mcp_mount_path if _MCP_APP is not None else "",
            "expose_agent_tools": settings.mcp["server"].get("expose_agent_tools", True),
            "expose_session_tools": settings.mcp["server"].get("expose_session_tools", True),
            "tool_count": len(mcp_list_tools()) if settings.mcp_server_enabled else 0,
        },
        "clients": tool_hub.status(),
        "capabilities": capabilities_overview(),
    }


@app.get("/api/mcp/tools")
async def mcp_tools():
    """列出本 MCP Server 对外暴露的全部工具。"""
    if not settings.mcp_server_enabled:
        return {"tools": []}
    return {
        "tools": [
            {"name": t.name, "description": t.description, "input_schema": t.inputSchema}
            for t in mcp_list_tools()
        ]
    }


@app.post("/api/mcp/clients")
async def mcp_connect(req: McpConnectReq):
    """运行时连接一个外部 MCP Server，其工具将注册为 mcp__<name>__<tool>。"""
    from .mcp.client import MCPConnectionError, tool_hub

    kwargs: dict = {}
    if req.url:
        kwargs["url"] = req.url
    if req.command:
        kwargs["command"] = req.command
        kwargs["args"] = req.args
        kwargs["env"] = req.env
    try:
        tools = await tool_hub.connect(req.name, req.transport, **kwargs)
    except MCPConnectionError as e:
        raise HTTPException(400, str(e))
    except ValueError as e:
        raise HTTPException(422, str(e))
    return {"ok": True, "name": req.name, "tools": tools}


@app.delete("/api/mcp/clients/{name}")
async def mcp_disconnect(name: str):
    """断开外部 MCP Server 并卸载其工具。"""
    from .mcp.client import tool_hub

    if not await tool_hub.disconnect(name):
        raise HTTPException(404, f"MCP server '{name}' 未连接")
    return {"ok": True, "disconnected": name}


if __name__ == "__main__":
    uvicorn.run("app.main:app", host=settings.host, port=settings.port)
