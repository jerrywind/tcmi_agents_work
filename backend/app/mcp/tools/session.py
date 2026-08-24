"""会话级 MCP 工具：把"完整一次中医问诊流程"暴露为 MCP 工具。

面向对话式 MCP 客户端（Claude Desktop / Cursor 等），带会话状态（cid），
全部复用现有 store / orchestrator，不引入额外状态。

工具清单：
    create_consultation / upload_image / upload_ppg / start_consultation /
    answer_question / get_state / get_report /
    list_families / create_family / add_member
"""
from __future__ import annotations

import base64
import uuid
from pathlib import Path

from mcp.types import Tool

from ...core.orchestrator import orchestrator
from ...models.schemas import ImageItem
from ...store import store

VALID_IMG = ("tongue", "face", "lesion", "palm_left", "palm_right")

_TOOLS: list[Tool] = [
    Tool(name="create_consultation",
         description="创建一次中医问诊（返回会话 cid）。参数：patient_age 年龄(int)，"
                     "patient_gender 性别('男'|'女'|'其他')，patient_region 地区(选填)，"
                     "complaint 主诉(选填)。",
         inputSchema={
             "type": "object",
             "properties": {
                 "patient_age": {"type": "integer", "description": "年龄"},
                 "patient_gender": {"type": "string", "enum": ["男", "女", "其他"]},
                 "patient_region": {"type": "string", "description": "地区，如 北京"},
                 "complaint": {"type": "string", "description": "主诉，如 乏力怕冷"},
             },
             "required": ["patient_age", "patient_gender"],
         }),
    Tool(name="upload_image",
         description="上传一张舌象/面相/患处/左手掌纹/右手掌纹照片。type 取值："
                     "tongue|face|lesion|palm_left|palm_right。图片通过 file_path(本地路径)"
                     "或 base64 提供。返回图片 url。",
         inputSchema={
             "type": "object",
             "properties": {
                 "cid": {"type": "string"},
                 "type": {"type": "string", "enum": list(VALID_IMG)},
                 "file_path": {"type": "string", "description": "本地图片路径"},
                 "base64": {"type": "string", "description": "base64 编码图片（与 file_path 二选一）"},
             },
             "required": ["cid", "type"],
         }),
    Tool(name="upload_ppg",
         description="上传/模拟 PPG 脉象信号。可传 samples+fs 真实采样，或用 simulate=true+profile "
                     "自动合成('normal'|'weak'|'slippery'|'tight'|'rapid')。返回解析结果(脉率/脉力/脉形等)。",
         inputSchema={
             "type": "object",
             "properties": {
                 "cid": {"type": "string"},
                 "samples": {"type": "array", "items": {"type": "number"}, "description": "PPG 采样序列"},
                 "fs": {"type": "integer", "description": "采样率 Hz，默认 50"},
                 "simulate": {"type": "boolean", "description": "true 时自动合成信号"},
                 "profile": {"type": "string", "description": "合成信号脉象特征"},
                 "rate_bpm": {"type": "number", "description": "合成信号脉率"},
             },
             "required": ["cid"],
         }),
    Tool(name="start_consultation",
         description="启动问诊：开始望闻问切 -> 辨证 Loop。sync=true 时同步跑完后再返回。",
         inputSchema={
             "type": "object",
             "properties": {
                 "cid": {"type": "string"},
                 "sync": {"type": "boolean", "description": "同步模式，默认 true"},
             },
             "required": ["cid"],
         }),
    Tool(name="answer_question",
         description="回答当前问诊问题。question_id 取 get_state 返回的 question.id，"
                     "text 为自由文本回答，value 为选项值(选填)。",
         inputSchema={
             "type": "object",
             "properties": {
                 "cid": {"type": "string"},
                 "question_id": {"type": "string"},
                 "value": {"type": "string", "description": "选项值"},
                 "text": {"type": "string", "description": "自由文本回答"},
                 "sync": {"type": "boolean", "description": "同步模式，默认 true"},
             },
             "required": ["cid", "question_id"],
         }),
    Tool(name="get_state",
         description="获取问诊当前状态：status/round/当前问题/证据池/辨证假设/报告。",
         inputSchema={
             "type": "object",
             "properties": {"cid": {"type": "string"}},
             "required": ["cid"],
         }),
    Tool(name="get_report",
         description="获取问诊最终辨证报告（status=finished 后可用）。",
         inputSchema={
             "type": "object",
             "properties": {"cid": {"type": "string"}},
             "required": ["cid"],
         }),
    Tool(name="list_families",
         description="列出已创建的家庭档案。",
         inputSchema={"type": "object", "properties": {}}),
    Tool(name="create_family",
         description="创建家庭档案。name 为家庭名称。返回 family_id。",
         inputSchema={
             "type": "object",
             "properties": {"name": {"type": "string"}},
             "required": ["name"],
         }),
    Tool(name="add_member",
         description="向家庭添加成员。relation 关系(如 本人/父亲)，patient 是否为患者，note 备注。",
         inputSchema={
             "type": "object",
             "properties": {
                 "fid": {"type": "string"},
                 "name": {"type": "string"},
                 "relation": {"type": "string"},
                 "patient": {"type": "boolean"},
                 "note": {"type": "string"},
             },
             "required": ["fid", "name", "relation"],
         }),
]

TOOL_NAMES = {t.name for t in _TOOLS}


def list_tools() -> list[Tool]:
    return list(_TOOLS)


def _read_image_bytes(file_path: str | None, b64: str | None) -> bytes:
    if file_path:
        return Path(file_path).read_bytes()
    if b64:
        return base64.b64decode(b64)
    raise ValueError("upload_image 需要 file_path 或 base64 之一")


async def handle_call(name: str, args: dict) -> dict | list | None:
    """处理会话级工具调用；工具名不属于本模块时返回 None。"""
    if name not in TOOL_NAMES:
        return None

    from ...main import _get, _state  # 延迟导入避免循环

    if name == "create_consultation":
        from ...models.schemas import Consultation, Patient
        pat = Patient(age=int(args["patient_age"]), gender=args["patient_gender"],
                      region=args.get("patient_region", ""))
        c = Consultation(patient=pat, complaint=args.get("complaint", ""))
        store.save(c)
        return {"cid": c.id, "status": c.status}

    if name == "upload_image":
        from ...config import UPLOAD_DIR
        cid = args["cid"]
        c = _get(cid)
        itype = args["type"]
        if itype not in VALID_IMG:
            raise ValueError(f"type 必须是 {VALID_IMG}")
        if c.status != "created":
            raise ValueError("consultation already started")
        data = _read_image_bytes(args.get("file_path"), args.get("base64"))
        fname = f"{cid}_{uuid.uuid4().hex[:8]}.jpg"
        dest = UPLOAD_DIR / fname
        dest.write_bytes(data)
        item = ImageItem(type=itype, path=str(dest), url=f"/uploads/{fname}")
        c.images.append(item)
        return {"id": item.id, "url": item.url, "type": itype}

    if name == "upload_ppg":
        from ...main import PpgReq, _handle_ppg
        c = _get(args["cid"])
        samples = args.get("samples") or []
        req = PpgReq(
            samples=samples,
            fs=int(args.get("fs", 50)),
            simulate=bool(args.get("simulate", False)) or not samples,
            profile=args.get("profile", "normal"),
            rate_bpm=(float(args["rate_bpm"]) if args.get("rate_bpm") is not None else 75.0),
        )
        return await _handle_ppg(c, req)

    if name == "start_consultation":
        c = _get(args["cid"])
        sync = bool(args.get("sync", True))
        if c.status == "created":
            if sync:
                await orchestrator.start_sync(c)
            else:
                await orchestrator.start(c)
        return _state(c).model_dump(mode="json")

    if name == "answer_question":
        c = _get(args["cid"])
        qid = args["question_id"]
        if c.current_question is None or c.current_question.id != qid:
            raise ValueError("question mismatch, 请先 get_state 获取最新 question.id")
        await orchestrator.answer(c, args.get("value", ""), args.get("text", ""),
                                  sync=bool(args.get("sync", True)))
        return _state(c).model_dump(mode="json")

    if name == "get_state":
        return _state(_get(args["cid"])).model_dump(mode="json")

    if name == "get_report":
        c = _get(args["cid"])
        if c.report is None:
            raise ValueError("report not ready")
        return c.report.model_dump(mode="json")

    if name == "list_families":
        return [f.model_dump(mode="json") for f in store.list_families()]

    if name == "create_family":
        from ...models.schemas import Family
        f = Family(name=args["name"])
        store.save_family(f)
        return {"family_id": f.id, "name": f.name}

    if name == "add_member":
        fid = args["fid"]
        fam = store.get_family(fid)
        if fam is None:
            raise ValueError("family not found")
        from ...models.schemas import Member
        m = Member(name=args["name"], relation=args["relation"],
                   note=args.get("note", ""))
        fam.members.append(m)
        return {"member_id": m.id, "family_id": fid}

    return None
