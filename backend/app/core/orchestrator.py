"""主诊 Agent（编排器）：驱动 望闻问切 -> 辨证 -> 终止判断/提问 的诊断 Loop。

编排器只依赖协议信封（AgentRequest/AgentResponse）与注册表 resolve()，
不 import 任何具体实现 —— 这是 sub-agent 可替换性的关键保证。
"""
from __future__ import annotations

import asyncio
import re
import uuid
from typing import Literal

from ..agents.listening import extract_keyword_evidences  # 仅用于把问答自由文本转证据
from ..config import settings
from ..knowledge.herb_safety import SAFETY_PREFIX, check_plan_safety
from ..knowledge.syndromes import ADVICE
from ..models.schemas import (CareTodo, CaseRecord, Consultation, Evidence, FollowUp,
                              Hypothesis, Message, Report, StreamSeg)
from ..protocol.base import AgentResponse, Capability
from ..protocol.registry import build_request, resolve
from ..skills.toolcall import run_tool_loop

# 后台任务注册表：task_id -> {"status": running|done|error, "error": str}
_RUNNING: dict[str, dict] = {}


def get_task(task_id: str) -> dict | None:
    return _RUNNING.get(task_id)


class Orchestrator:
    def __init__(self) -> None:
        cfg = settings.loop
        self.max_rounds: int = cfg["max_rounds"]
        self.single_conf: float = cfg["single_conf"]
        self.single_gap: float = cfg["single_gap"]
        self.dual_conf: float = cfg["dual_conf"]
        self.min_evidences: int = cfg["min_evidences"]

    # ---------- 内部工具 ----------
    async def _call(self, c: Consultation, cap: Capability, payload: dict) -> AgentResponse:
        agent, route = resolve(cap)
        # 推理成本自适应路由：早期/低证据任务走低算力轻模型，晚期/高证据切主模型
        llm_cfg = settings.llm
        if llm_cfg.get("adaptive_route") and route.get("impl") == "llm":
            light = llm_cfg.get("light_model") or "text-default"
            thr_e = int(llm_cfg.get("light_threshold_evidences", 3))
            thr_r = int(llm_cfg.get("light_threshold_round", 1))
            if len(c.evidences) < thr_e and c.round <= thr_r:
                route = {**route, "model": light}
        req = build_request(cap, session_id=c.id, round=c.round, payload=payload,
                            model=route.get("model", ""),
                            evidences=c.evidences, hypotheses=c.hypotheses,
                            asked_keys=c.asked_keys)
        resp = await agent.run(req)
        # 可观测性：标注降级（两种口径）与 token 用量
        #  1) resolve 级：期望实现未注册 -> 回退 rule（requested_impl != 实际 impl）
        #  2) 运行时级：LLM 实现运行中无 key/provider 失败并内部回退规则
        #     （此时对外 impl 仍标 llm，由 resp.meta["degraded"] 显式标记）
        requested_impl = route.get("requested_impl")
        actual_impl = resp.meta.get("impl")
        resolve_degraded = bool(requested_impl) and requested_impl != actual_impl
        runtime_degraded = bool(resp.meta.get("degraded"))
        degraded = resolve_degraded or runtime_degraded
        if degraded:
            if resolve_degraded:
                reason = f"路由期望 {requested_impl}，实际降级为 {actual_impl}"
            else:
                reason = resp.notes or "LLM 实现运行时降级为规则兜底"
        else:
            reason = None
        trace_entry = {"round": c.round, "capability": cap.value,
                       "impl": actual_impl, "requested_impl": requested_impl,
                       "status": resp.status,
                       "latency_ms": resp.meta.get("latency_ms"),
                       "tokens": resp.meta.get("tokens"),
                       "degraded": degraded if degraded else None,
                       "degraded_reason": reason,
                       "error": resp.error or None}
        c.trace.append(trace_entry)
        return resp

    @staticmethod
    def _merge_evidences(c: Consultation, new: list[Evidence]) -> None:
        by_key = {e.key: i for i, e in enumerate(c.evidences)}
        for ev in new:
            if ev.key in by_key:
                old = c.evidences[by_key[ev.key]]
                if ev.confidence >= old.confidence:   # 高置信度覆盖
                    c.evidences[by_key[ev.key]] = ev
            else:
                by_key[ev.key] = len(c.evidences)
                c.evidences.append(ev)

    @staticmethod
    def _say(c: Consultation, content: str, type_: Literal["text", "question", "report", "alert"] = "text",
             role: Literal["agent", "user", "system"] = "agent") -> None:
        c.messages.append(Message(role=role, type=type_, content=content))
        # 同步推入实时流式分段（前端 after=seq 增量拉取）
        c.stream_seq += 1
        c.stream.append(StreamSeg(seq=c.stream_seq, role=role,
                                  type=type_, content=content, done=True))

    @staticmethod
    def _stream_begin(c: Consultation, type_: Literal["text", "question", "report", "alert"] = "text",
                      role: Literal["agent", "user", "system"] = "agent") -> int:
        """开一条流式消息，返回 seq；后续用 _stream_chunk 逐段追加。"""
        c.stream_seq += 1
        c.stream.append(StreamSeg(seq=c.stream_seq, role=role,
                                  type=type_, content="", done=False))
        return c.stream_seq

    @staticmethod
    def _stream_chunk(c: Consultation, seq: int, text: str,
                      done: bool = False) -> None:
        """向某条流式消息追加文本；done=True 时该消息标记完成（同时落一条 Message）。"""
        target = next((s for s in reversed(c.stream) if s.seq == seq), None)
        if target is None:
            return
        target.content += text
        target.done = done
        if done:
            c.messages.append(Message(role=target.role, type=target.type,
                                      content=target.content))  # type: ignore[arg-type]

    # ---------- 流程入口 ----------
    async def start(self, c: Consultation) -> Consultation:
        if c.status != "created":
            return c
        task_id = uuid.uuid4().hex
        _RUNNING[task_id] = {"status": "running", "error": None}
        c.meta["task_id"] = task_id
        c.status = "running"
        asyncio.create_task(self._run_start(c, task_id))
        return c

    async def _run_start(self, c: Consultation, task_id: str) -> None:
        try:
            await self._start_impl(c)
        except Exception as exc:  # 后台任务不能抛到事件循环
            _RUNNING[task_id] = {"status": "error", "error": str(exc)}
            self._say(c, f"（问诊引擎异常：{exc}）", type_="alert")
            raise
        _RUNNING[task_id] = {"status": "done", "error": None}

    async def start_sync(self, c: Consultation) -> Consultation:
        """同步模式：直接跑完诊断 Loop（测试 / 非流式场景）。"""
        if c.status != "created":
            return c
        c.status = "running"
        await self._start_impl(c)
        return c
        _RUNNING[task_id] = {"status": "done", "error": None}

    async def _start_impl(self, c: Consultation) -> None:
        self._say(c, c.complaint, role="user")

        # 1) 安全检查（自述）
        if await self._safety_gate(c, c.complaint):
            return

        # 2) 望 / 闻 / 切 并行采集证据
        payload_common = {"self_report": c.self_report, "gender": c.patient.gender}
        palp_payload = dict(payload_common)
        if c.ppg is not None:
            palp_payload["ppg"] = c.ppg.model_dump()
        insp, listen, palp = await asyncio.gather(
            self._call(c, Capability.INSPECTION, {
                "images": [{"type": i.type, "path": i.path} for i in c.images],
                **payload_common}),
            self._call(c, Capability.LISTENING, {"text": c.complaint}),
            self._call(c, Capability.PALPATION, palp_payload),
        )
        for resp in (insp, listen, palp):
            if resp.status == "ok":
                self._merge_evidences(c, resp.evidences)
            if resp.notes:
                self._say(c, resp.notes)

        # 3) 进入诊断 loop
        await self._loop_step(c)

    async def answer(self, c: Consultation, value: str, text: str, sync: bool = False) -> Consultation:
        # 诊疗方案个性化追问阶段
        if c.status == "treatment_qa" and c.current_question is not None:
            if text and await self._safety_gate(c, text):
                return c
            q = c.current_question
            display = value or text or "(未填)"
            self._say(c, display, role="user")
            c.treatment_answers.append({"key": q.key, "value": value or text or ""})
            c.current_question = None
            c.status = "planning"
            if sync:
                await self._run_treatment(c, None)
            else:
                task_id = uuid.uuid4().hex
                _RUNNING[task_id] = {"status": "running", "error": None}
                c.meta["task_id"] = task_id
                asyncio.create_task(self._run_treatment(c, task_id))
            return c
        # 辨证追问阶段
        if c.status != "waiting_answer" or c.current_question is None:
            return c
        q = c.current_question
        display = value or text
        self._say(c, display, role="user")

        # 红旗检查（自由文本）
        if text and await self._safety_gate(c, text):
            return c

        if sync:
            await self._run_answer(c, None, value, text)
        else:
            task_id = uuid.uuid4().hex
            _RUNNING[task_id] = {"status": "running", "error": None}
            c.meta["task_id"] = task_id
            asyncio.create_task(self._run_answer(c, task_id, value, text))
        return c

    async def _run_answer(self, c: Consultation, task_id: str | None, value: str, text: str) -> None:
        try:
            q = c.current_question
            if q is None:
                return
            # 结构化答案入池
            if value:
                self._merge_evidences(c, [Evidence(key=q.key, value=value,
                                                   source="问", confidence=0.9, round=c.round)])
            # 自由文本再走一次关键词抽取（复用闻诊逻辑）
            if text:
                self._merge_evidences(c, extract_keyword_evidences(text, c.round, source="问"))
            c.asked_keys.append(q.key)
            c.current_question = None
            c.status = "running"
            await self._loop_step(c)
        except Exception as exc:
            if task_id is not None:
                _RUNNING[task_id] = {"status": "error", "error": str(exc)}
            self._say(c, f"（问诊引擎异常：{exc}）", type_="alert")
            raise
        if task_id is not None:
            _RUNNING[task_id] = {"status": "done", "error": None}

    async def _run_treatment(self, c: Consultation, task_id: str | None) -> None:
        try:
            await self._treatment_step(c)
        except Exception as exc:
            if task_id is not None:
                _RUNNING[task_id] = {"status": "error", "error": str(exc)}
            self._say(c, f"（问诊引擎异常：{exc}）", type_="alert")
            raise
        if task_id is not None:
            _RUNNING[task_id] = {"status": "done", "error": None}

    # ---------- Loop 单步：辨证 -> 终止判断 -> 提问/出报告 ----------
    async def _loop_step(self, c: Consultation) -> Consultation:
        diff = await self._call(c, Capability.DIFFERENTIATION, {})
        if diff.status == "ok" and diff.hypotheses:
            c.hypotheses = diff.hypotheses

        final = self._pick_final(c)
        if final is not None:
            return await self._finish(c, final)

        if c.round >= self.max_rounds:
            return await self._finish(c, c.hypotheses[:2], forced=True)

        inq = await self._call(c, Capability.INQUIRY,
                               {"gender": c.patient.gender})
        if inq.status != "ok" or inq.question is None:
            return await self._finish(c, c.hypotheses[:2], forced=True)

        c.round += 1
        c.current_question = inq.question
        c.status = "waiting_answer"
        self._say(c, inq.question.text, type_="question")
        return c

    def _pick_final(self, c: Consultation) -> list[Hypothesis] | None:
        hyps = c.hypotheses
        if len(c.evidences) < self.min_evidences or not hyps:
            return None
        top1 = hyps[0]
        top2 = hyps[1] if len(hyps) > 1 else None
        gap = top1.confidence - (top2.confidence if top2 else 0.0)
        if top1.confidence >= self.single_conf and gap >= self.single_gap:
            return [top1]
        if (top2 and top1.confidence >= self.dual_conf
                and top2.confidence >= self.dual_conf and gap < self.single_gap
                and len(c.evidences) >= self.min_evidences + 1):
            return [top1, top2]     # 兼证
        return None

    # 证候常见传变方向（经验归纳，用于提示而非诊断）：证候名 -> 可能的发展/转归
    _EVOLUTION: dict[str, str] = {
        "风寒犯肺": "表寒不解可化热入里，演变为风热或痰热壅肺，需注意发热、痰色转黄。",
        "风热犯肺": "热邪易伤津炼液成痰，可转为痰热咳嗽或咽痛加重。",
        "脾胃湿热": "湿热缠绵易困脾，久则脾虚生痰，或湿热下注致大便黏滞不爽。",
        "肝郁脾虚": "肝郁化火可犯胃（吞酸、胁痛），或脾虚加重致气血生化不足、乏力。",
        "心脾两虚": "气血亏虚日久可及肾，出现畏寒、腰膝酸软等心脾肾同调之象。",
        "肾阴虚": "阴虚火旺可上扰心神致失眠加重，或虚火灼津成瘀。",
        "肾阳虚": "命门火衰可累及脾阳，出现五更泄泻、完谷不化。",
        "痰湿蕴肺": "痰湿久蕴可化热，或碍脾生湿，形成痰湿互结。",
        "气血两虚": "气血不足推动无力，久则兼血瘀，症见面色晦暗、舌质暗。",
    }

    @staticmethod
    def _evolution_hint(finals: list[Hypothesis]) -> str:
        hints = [Orchestrator._EVOLUTION.get(h.name) for h in finals]
        hints = [h for h in hints if h]
        return "" if not hints else "；".join(hints)

    async def _finish(self, c: Consultation, finals: list[Hypothesis],
                forced: bool = False) -> Consultation:
        finals = [h for h in finals if h.confidence > 0][:2]
        # 证据 key -> 来源映射，用于溯源标注
        src_map = {e.key: e.source for e in c.evidences}
        tagged = lambda keys: "、".join(f"{k}（{src_map.get(k, '问')}）" for k in keys) or "无"
        if not finals:
            c.report = Report(
                syndromes=[],
                reasoning="现有信息不足以形成可靠辨证结论，建议携带舌象照片线下面诊。",
            )
        else:
            chains = []
            advice: dict = {}
            for h in finals:
                chains.append(f"【{h.name}】支持证据：{tagged(h.supporting)}"
                              + (f"；矛盾证据：{tagged(h.contradicting)}" if h.contradicting else ""))
                for k, v in ADVICE.get(h.name, {}).items():
                    advice[k] = (advice.get(k, "") + ("；" if k in advice else "") + v)
            prefix = "（已达最大问询轮数，给出当前最可能方向）" if forced else ""
            reasoning = prefix + "\n".join(chains)
            # 从辨证依据中提取知识出处（书名号），用于报告引用标注
            sources = sorted({m for m in re.findall(r"《([^》]+)》", reasoning)})
            # 证候传变提示：基于当前证候的常见发展方向，辅助调护方向判断
            evolution = self._evolution_hint(finals)
            c.report = Report(syndromes=finals,
                              reasoning=reasoning, advice=advice, sources=sources,
                              evolution=evolution)
        c.status = "planning"
        names = "、".join(h.name for h in finals) or "暂无法确定"
        self._say(c, f"辨证完成：{names}。正在为您制定诊疗方案……", type_="report")
        return await self._treatment_step(c)

    # ---------- 诊疗方案阶段 ----------
    @staticmethod
    def _derive_care(plans: list) -> list:
        """由诊疗方案派生可执行待办（打卡/煎药提醒/预约）。"""
        todos: list = []
        for p in plans:
            cat = getattr(p, "category", "")
            title = getattr(p, "title", "") or cat
            detail = getattr(p, "detail", "")
            if cat == "中药方剂":
                todos.append(CareTodo(title=f"{title} · 煎服", category=cat,
                                      detail=detail, kind="decoct",
                                      times=["08:00", "20:00"]))
            elif cat in ("针灸推拿", "外治法"):
                todos.append(CareTodo(title=f"{title}", category=cat,
                                      detail=detail,
                                      kind="appointment" if cat == "针灸推拿" else "checkin"))
            else:  # 生活调护 / 膳食 / 西医检查
                todos.append(CareTodo(title=f"{title}", category=cat,
                                      detail=detail, kind="checkin",
                                      times=["09:00"]))
        return todos

    @staticmethod
    def _derive_followups(plans: list) -> list:
        """由诊疗方案生成随访回访点（3/7/14 天）。"""
        cats = {getattr(p, "category", "") for p in plans}
        focus_parts = []
        if "中药方剂" in cats:
            focus_parts.append("症状缓解程度与服药后反应")
        if "针灸推拿" in cats or "外治法" in cats:
            focus_parts.append("外治/针灸后的局部反应")
        focus_parts.append("睡眠、饮食、二便等整体变化")
        focus = "；".join(focus_parts)
        return [FollowUp(due_in_days=d, focus=focus) for d in (3, 7, 14)]

    @staticmethod
    def _save_case(c: Consultation) -> None:
        """自动沉淀脱敏医案（去隐私：不含姓名/联系方式），供 RAG / 教学反哺。"""
        try:
            from pathlib import Path
            cases_dir = Path(__file__).resolve().parents[2] / "cases"
            cases_dir.mkdir(exist_ok=True)
            rec = CaseRecord(
                complaint=c.complaint,
                gender=c.patient.gender,
                syndromes=[h.name for h in (c.report.syndromes if c.report else [])],
                evidences=[e.model_dump() for e in c.evidences],
                treatments=[p.model_dump() for p in (c.report.treatments if c.report else [])],
                evolution=c.report.evolution if c.report else "",
            )
            with open(cases_dir / "cases.jsonl", "a", encoding="utf-8") as f:
                f.write(rec.model_dump_json() + "\n")
        except Exception:
            pass  # 沉淀失败不应影响问诊主流程

    async def followup_feedback(self, c: Consultation, fid: str, text: str) -> Consultation:
        """用户回访反馈 → 回流证据池 + 辨证微调提示。"""
        fu = next((f for f in c.followups if f.id == fid), None)
        if fu is None:
            return c
        fu.done = True
        fu.feedback = text
        self._say(c, text, role="user")
        # 反馈转证据（复用闻诊关键词抽取）
        evs = extract_keyword_evidences(text, round_=c.round + 1, source="随访")
        if evs:
            self._merge_evidences(c, evs)
            self._say(c, f"已记录您的回访反馈，并更新辨证证据（{len(evs)} 条）。"
                      f"若症状持续或加重，建议线下面诊调整方案。")
        else:
            self._say(c, "已记录您的回访反馈，感谢配合。")
        return c

    async def lab_interpret(self, c: Consultation, text: str) -> dict:
        """中西证据融合：解析西医检验报告文本，给出指标异常 + 中医证候倾向交叉解读。

        返回 {tcm_note, indicators, evidence_keys}，并把结构化证据汇入证据池。
        """
        c.lab_reports.append(text)
        from ..protocol.base import Capability
        from ..protocol.llm import get_provider, parse_json
        prompt = (
            "你是一名中西医结合分析师。下面是用户的西医检验报告/指标文本。"
            "请提取异常指标（含数值与单位），并给出可能的中医证候倾向（与现有症状互参）。\n"
            "只输出 JSON：{\"indicators\":[{\"name\":str,\"value\":str,\"abnormal\":bool}],"
            "\"tcm_note\":str}。不要编造指标。\n\n报告文本：\n" + text
        )
        try:
            raw, _ = await run_tool_loop(
                get_provider(),
                [{"role": "user", "content": prompt}],
                None,
                Capability.DIFFERENTIATION.value,
            )
            data = parse_json(raw)
        except Exception:
            data = None
        if not isinstance(data, dict):
            self._say(c, "检验报告已记录，但当前模型暂无法解析，建议携带报告线下面诊。")
            return {"tcm_note": "", "indicators": [], "evidence_keys": []}
        indicators = data.get("indicators") or []
        tcm_note = data.get("tcm_note") or ""
        # 异常指标作为'检'来源证据，参与辨证
        evs = []
        for ind in indicators:
            if ind.get("abnormal"):
                evs.append(Evidence(key=f"lab.{ind.get('name', '指标')}",
                                    value=str(ind.get("value", "")),
                                    source="检", confidence=0.8, round=c.round))
        if evs:
            self._merge_evidences(c, evs)
        summary = "检验报告解读：" + (tcm_note or "未见明显异常指标。")
        self._say(c, summary)
        return {"tcm_note": tcm_note, "indicators": indicators,
                "evidence_keys": [e.key for e in evs]}

    async def _treatment_step(self, c: Consultation) -> Consultation:
        """诊断完成后的诊疗方案阶段：必要时个性化追问 1~2 条，否则出方案。"""
        if c.report is None:
            c.report = Report()
        top = [h.name for h in c.report.syndromes]
        resp = await self._call(
            c, Capability.TREATMENT,
            {"diagnoses": top,
             "patient": c.patient.model_dump(),
             "qa": c.treatment_answers})
        if resp.status == "ok" and resp.question is not None:
            c.current_question = resp.question
            c.status = "treatment_qa"
            self._say(c, resp.question.text, type_="question")
            return c
        if resp.status == "ok" and resp.plans:
            # 用药安全硬校验（十八反/十九畏/孕忌），规则优先，不依赖 LLM
            pregnancy = next((a["value"] for a in c.treatment_answers
                              if a.get("key") == "treat.pregnancy"
                              and "孕" in str(a.get("value", ""))), None)
            warnings_total = 0
            for p in resp.plans:
                if p.category != "中药方剂" or not p.detail:
                    continue
                ws = check_plan_safety(p.detail, pregnant=bool(pregnancy))
                if ws:
                    p.warnings = ws
                    warnings_total += len(ws)
                    extra = " ".join(ws)
                    p.note = (p.note + " " if p.note else "") + SAFETY_PREFIX + extra
            if warnings_total:
                self._say(c, f"已对方案做用药安全校验，发现 {warnings_total} 处配伍/禁忌提示，"
                          f"已在报告中标注。", type_="alert")
            c.report.treatments = resp.plans
            c.care_todos = self._derive_care(c.report.treatments)
            c.followups = self._derive_followups(c.report.treatments)
            c.status = "finished"
            self._save_case(c)
            self._say(c, f"已结合您的情况制定 {len(resp.plans)} 项诊疗方案，请查看报告。",
                      type_="report")
            return c
        c.status = "finished"
        self._save_case(c)
        self._say(c, "诊疗方案生成失败，请线下就医获取专业方案。", type_="report")
        return c

    # ---------- 安全 ----------
    async def _safety_gate(self, c: Consultation, text: str) -> bool:
        resp = await self._call(c, Capability.SAFETY, {"text": text})
        if resp.alerts:
            a = resp.alerts[0]
            c.report = Report(syndromes=[], reasoning=a.reason, red_flag=a.reason,
                              advice={"紧急建议": a.advice})
            c.status = "referred"
            self._say(c, f"⚠ {a.reason} {a.advice}", type_="alert")
            return True
        return False


orchestrator = Orchestrator()
