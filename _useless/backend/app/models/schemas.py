"""领域数据模型：档案、证据、证候假设、问题、报告、会话。"""
from __future__ import annotations

import time
import uuid
from typing import Literal, Optional

from pydantic import BaseModel, Field


def _uid(prefix: str = "") -> str:
    return prefix + uuid.uuid4().hex[:10]


# ---------- 档案 ----------
class Patient(BaseModel):
    region: str = ""          # 常住地
    height_cm: float = 0
    weight_kg: float = 0
    age: int = 0
    gender: str = "未知"      # 男 | 女 | 未知


# ---------- PPG 脉象（硬件接入/模拟信号解析） ----------
class PpgReading(BaseModel):
    rate_bpm: float = 0.0
    rhythm: str = "整齐"                       # 整齐 | 不齐 | 结代
    depth: str = "中"                          # 浮 | 中 | 沉
    force: str = "有力"                        # 有力 | 无力 | 和缓
    shape: str = "平"                          # 滑 | 涩 | 平
    amplitude: float = 0.0
    perfusion: float = 0.0
    signal_quality: float = 0.0               # 0~1
    notes: str = ""
    ts: float = Field(default_factory=time.time)


# ---------- 家庭与成员（一人管理全家档案，多租户准备） ----------
class Member(BaseModel):
    id: str = Field(default_factory=lambda: _uid("m_"))
    family_id: str = ""                       # 所属家庭
    name: str = ""                            # 称呼，如 本人/父亲/女儿
    relation: str = "本人"                    # 与户主关系：本人/父亲/母亲/子女/配偶/其他
    patient: Patient = Patient()              # 该成员体质档案
    note: str = ""                            # 备注（过敏史/慢病等）
    created_at: float = Field(default_factory=time.time)


class Family(BaseModel):
    id: str = Field(default_factory=lambda: _uid("f_"))
    name: str = "我的家庭"                     # 家庭名称
    owner: str = ""                           # 户主标识（手机号/昵称，留空可匿名）
    members: list[Member] = []                # 家庭成员
    created_at: float = Field(default_factory=time.time)


class ImageItem(BaseModel):
    id: str = Field(default_factory=lambda: _uid("img_"))
    type: Literal["tongue", "face", "lesion", "palm_left", "palm_right"]
    url: str = ""             # 对外访问 url
    path: str = ""            # 服务器本地路径（供多模态模型读取）
    analysis: Optional[dict] = None


# ---------- 证据与假设 ----------
class Evidence(BaseModel):
    id: str = Field(default_factory=lambda: _uid("ev_"))
    key: str                                  # 特征键，如 tongue.coat / sweat
    value: str                                # 特征值，如 黄腻 / 盗汗
    source: Literal["望", "闻", "问", "切", "自述", "检"] = "问"
    confidence: float = 0.8
    round: int = 0
    desc: str = ""


class Hypothesis(BaseModel):
    name: str                                 # 证候名，如 脾胃湿热
    confidence: float = 0.0
    supporting: list[str] = []                # 支持证据描述
    contradicting: list[str] = []             # 矛盾证据描述


# ---------- 诊疗方案 ----------
class TreatmentPlan(BaseModel):
    id: str = Field(default_factory=lambda: _uid("tp_"))
    category: Literal["中药方剂", "针灸推拿", "外治法", "西医检查", "生活调护", "膳食"] = "生活调护"
    title: str = ""                           # 方案标题，如 "三仁汤加减"
    detail: str = ""                          # 具体内容（方剂组成/操作/检查项目）
    rationale: str = ""                       # 选择该方案的辨证依据
    note: str = ""                            # 注意事项 / 适用条件 / 禁忌
    warnings: list[str] = []                  # 用药安全硬校验警示（十八反/十九畏/孕忌等）
    priority: int = 1                         # 1=首选，数字越大优先级越低


# ---------- 问题 ----------
class QuestionOption(BaseModel):
    label: str
    value: str


class Question(BaseModel):
    id: str = Field(default_factory=lambda: _uid("q_"))
    key: str                                  # 对应特征键
    text: str
    options: list[QuestionOption] = []
    allow_free_text: bool = True


# ---------- 报告 ----------
class Report(BaseModel):
    syndromes: list[Hypothesis] = []          # 1~2 条最终诊断
    reasoning: str = ""                       # 辨证依据链
    advice: dict = {}                         # {饮食, 起居, 建议就诊科室}
    treatments: list[TreatmentPlan] = []      # 诊疗方案（开方/针灸/西医检查等）
    red_flag: Optional[str] = None            # 红旗告警（若有则建议立即就医）
    sources: list[str] = []                    # 参考来源（知识库出处，如《伤寒论》），从辨证依据中归一化
    evolution: str = ""                        # 证候传变提示（基于当前证候的常见发展方向）
    disclaimer: str = "本结果由 AI 生成，仅供健康参考，不构成医疗诊断或处方建议。如有不适请及时线下就医。"


# ---------- 可执行诊疗待办（打卡/提醒） ----------
class CareTodo(BaseModel):
    id: str = Field(default_factory=lambda: _uid("ct_"))
    title: str = ""                           # 待办标题，如 "三仁汤 煎服"
    category: str = ""                         # 对应方案类别
    detail: str = ""                           # 具体内容（方剂组成/操作）
    kind: Literal["decoct", "checkin", "appointment"] = "checkin"
    times: list[str] = []                      # 提醒时刻，如 ["08:00", "20:00"]
    done: bool = False                         # 今日是否已完成打卡


# ---------- 随访回访计划 ----------
class FollowUp(BaseModel):
    id: str = Field(default_factory=lambda: _uid("fu_"))
    due_in_days: int                          # 距首诊天数（3/7/14...）
    focus: str = ""                           # 回访关注点，如 "胃胀/睡眠改善情况"
    done: bool = False                        # 是否已回访
    feedback: str = ""                        # 用户反馈文本


# ---------- 舌象/面象复诊时序对比 ----------
class RevisitImage(BaseModel):
    id: str = Field(default_factory=lambda: _uid("rv_"))
    ts: float = Field(default_factory=time.time)
    path: str = ""                            # 复诊图片路径/URL
    kind: str = "tongue"                      # tongue / face
    features: dict = {}                       # 望诊结构化特征，如 {tongue.body: 红, tongue.coat: 黄腻}


# ---------- 自动沉淀医案（脱敏，反哺 RAG / 教学） ----------
class CaseRecord(BaseModel):
    id: str = Field(default_factory=lambda: _uid("case_"))
    ts: float = Field(default_factory=time.time)
    complaint: str = ""                       # 主诉（脱敏，无个人信息）
    gender: str = ""
    syndromes: list[str] = []                 # 辨证结论
    evidences: list[dict] = []                # 证据（去隐私）
    treatments: list[dict] = []               # 方案（类别/标题/组成）
    evolution: str = ""


# ---------- 会话消息（供前端聊天流展示） ----------
class Message(BaseModel):
    id: str = Field(default_factory=lambda: _uid("m_"))
    role: Literal["agent", "user", "system"]
    type: Literal["text", "question", "report", "alert"] = "text"
    content: str = ""
    ts: float = Field(default_factory=time.time)


# ---------- 实时流式分段（前端逐字/逐段渲染用） ----------
class StreamSeg(BaseModel):
    seq: int                                   # 自增序号，前端用 after=seq 拉增量
    role: Literal["agent", "user", "system"] = "agent"
    type: Literal["text", "question", "report", "alert"] = "text"
    content: str = ""                          # 本段文本（可能是整句或逐字增量）
    done: bool = False                         # 该条消息是否完整（用于的分段聚合）


# ---------- 问诊会话 ----------
class Consultation(BaseModel):
    id: str = Field(default_factory=lambda: _uid("c_"))
    ts: float = Field(default_factory=time.time)  # 创建时间（时序/列表排序用）
    family_id: str = ""                        # 所属家庭（家庭档案功能）
    member_id: str = ""                        # 所属家庭成员（家庭档案功能）
    patient: Patient
    complaint: str = ""                       # 病情自述
    self_report: dict = {}                    # 可选自测项（如心率）
    images: list[ImageItem] = []
    ppg: Optional[PpgReading] = None             # PPG 脉象解析结果（硬件/模拟）
    status: Literal["created", "running", "waiting_answer", "planning",
                     "treatment_qa", "finished", "referred"] = "created"
    round: int = 0
    evidences: list[Evidence] = []
    hypotheses: list[Hypothesis] = []
    current_question: Optional[Question] = None
    asked_keys: list[str] = []                # 已问过的特征键，避免重复提问
    treatment_answers: list[dict] = []        # 诊疗方案个性化追问的答案 {key, value}
    report: Optional[Report] = None
    messages: list[Message] = []
    trace: list[dict] = []                    # sub-agent 调用轨迹（可观测性）
    task_id: Optional[str] = None             # 当前后台任务 id（流式轮询用）
    meta: dict = {}                           # 扩展字段（兼容动态写入）
    care_todos: list["CareTodo"] = []         # 由诊疗方案派生的可执行待办（打卡/提醒）
    followups: list["FollowUp"] = []          # 随访回访计划（自动生成）
    revisits: list["RevisitImage"] = []       # 复诊舌象/面象（时序对比用）
    lab_reports: list[str] = []               # 西医检验报告文本（供中西证据融合解读）

    # 实时流式分段：orchestrator 边说边推，前端用 after=seq 增量拉取
    stream: list[StreamSeg] = []
    stream_seq: int = 0                        # 内部游标，随对象持久化


# ---------- API DTO ----------
class CreateConsultationReq(BaseModel):
    patient: Patient
    complaint: str
    self_report: dict = {}
    family_id: str = ""                        # 选填：归属家庭
    member_id: str = ""                        # 选填：归属成员


class AnswerReq(BaseModel):
    question_id: str
    value: str = ""                           # 选中的选项值
    text: str = ""                            # 自由文本补充


class StateResp(BaseModel):
    id: str
    status: str
    round: int
    family_id: str = ""
    member_id: str = ""
    ppg: Optional[PpgReading] = None
    evidences: list[Evidence] = []
    question: Optional[Question] = None
    hypotheses: list[Hypothesis] = []
    messages: list[Message] = []
    report: Optional[Report] = None
    task_id: Optional[str] = None          # 后台任务 id，用于轮询流式分段
