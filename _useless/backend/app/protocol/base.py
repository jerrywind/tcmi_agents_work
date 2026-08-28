"""
Sub-Agent 协议层（核心）
========================
目标：任何一个诊断子项（望/闻/问/切/辨证/安全）的实现都可以被独立替换
（换规则引擎、换 LLM、换模型、甚至换远程服务），编排器不感知具体实现。

协议要点：
1. 统一信封：AgentRequest / AgentResponse（强类型 Pydantic），
   实现方只消费 request.payload + 证据快照，只产出结构化结果。
2. 声明式元数据：每个实现声明 capability + impl_name + 输入需求，
   通过注册表 + routing.yaml 完成解析，切换实现 = 改一行配置。
3. 无状态约定：Sub-Agent 不允许持有会话状态，所有状态经信封传递，
   因此可以随时横向替换/并行调用/远程化（信封天然可 JSON 序列化）。
4. 统一容错：run() 包装 handle()，超时/异常降级为 status=error，
   编排器可按 capability 决定 fallback 策略。
"""
from __future__ import annotations

import time
import traceback
import uuid
from abc import ABC, abstractmethod
from enum import Enum
from typing import ClassVar, Optional

from pydantic import BaseModel, Field

from ..models.schemas import Evidence, Hypothesis, Question, TreatmentPlan


class Capability(str, Enum):
    INSPECTION = "diagnosis.inspection"          # 望
    LISTENING = "diagnosis.listening"            # 闻
    INQUIRY = "diagnosis.inquiry"                # 问
    PALPATION = "diagnosis.palpation"            # 切
    DIFFERENTIATION = "diagnosis.differentiation"  # 辨证
    SAFETY = "diagnosis.safety"                  # 安全
    TREATMENT = "treatment.plan"                 # 诊疗方案（开方/针灸/西医检查等）


class AgentRequest(BaseModel):
    """标准请求信封：编排器 -> Sub-Agent。"""
    request_id: str = Field(default_factory=lambda: uuid.uuid4().hex[:12])
    capability: Capability
    session_id: str = ""
    round: int = 0
    payload: dict = {}                 # capability 特定输入（图像路径/文本/心率...）
    evidences: list[Evidence] = []     # 证据池快照（只读）
    hypotheses: list[Hypothesis] = []  # 当前候选证候快照（只读）
    asked_keys: list[str] = []         # 已提问过的特征键
    options: dict = {}                 # 路由配置透传的实现参数
    model: str = ""                    # 路由指定的逻辑模型名


class Alert(BaseModel):
    level: str = "danger"              # danger | warning
    reason: str = ""
    advice: str = ""


class AgentResponse(BaseModel):
    """标准响应信封：Sub-Agent -> 编排器。"""
    request_id: str = ""
    capability: Capability
    status: str = "ok"                 # ok | error | skip
    evidences: list[Evidence] = []     # 新增证据（望/闻/切/问答解析产出）
    hypotheses: list[Hypothesis] = []  # 辨证 agent 产出
    question: Optional[Question] = None  # 问诊/诊疗方案 agent 产出（追问）
    plans: list[TreatmentPlan] = []    # 诊疗方案 agent 产出
    alerts: list[Alert] = []           # 安全 agent 产出
    notes: str = ""                    # 给用户/日志的说明
    error: str = ""
    meta: dict = {}                    # {impl, model, latency_ms, tokens} 可观测性


class SubAgent(ABC):
    """所有 Sub-Agent 实现的基类。子类必须声明 capability 与 impl_name。"""

    capability: ClassVar[Capability]
    impl_name: ClassVar[str] = "rule"
    description: ClassVar[str] = ""

    async def run(self, req: AgentRequest) -> AgentResponse:
        """统一入口：计时 + 异常兜底，保证编排器拿到合法信封。"""
        t0 = time.perf_counter()
        try:
            resp = await self.handle(req)
        except Exception as exc:  # noqa: BLE001
            resp = AgentResponse(
                capability=self.capability, status="error",
                error=f"{type(exc).__name__}: {exc}",
                notes=traceback.format_exc(limit=3),
            )
        resp.request_id = req.request_id
        resp.meta = {
            **resp.meta,
            "impl": self.impl_name,
            "model": req.model,
            "latency_ms": round((time.perf_counter() - t0) * 1000, 1),
        }
        return resp

    @abstractmethod
    async def handle(self, req: AgentRequest) -> AgentResponse:
        ...
