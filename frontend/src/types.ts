// 类型定义：仅保留 harness 契约 + 前端自持的本地数据模型。
//
// 说明：原 backend 的会话式模型（ConsultState / Report / CareTodo / FollowUp /
// RevisitCompare / LabResult / PpgReading / Family ...）随旧 backend 一并移除。
// harness 是**无状态**服务，不保存会话、不提供上述任何能力，
// 因此这些类型若保留只会造成「前端以为有、后端其实没有」的错觉。

import type {
  DiagnosisResult,
  DiagnosisStep,
  DifferentiationStructured,
  HarnessCapability,
  HarnessMessage,
  HarnessSkill,
  ReportMeta,
  ReportsResult,
  StoredReport,
  SyndromeAssessment,
} from './services/harness'

export type {
  DiagnosisResult,
  DiagnosisStep,
  DifferentiationStructured,
  HarnessCapability,
  HarnessMessage,
  HarnessSkill,
  ReportMeta,
  ReportsResult,
  StoredReport,
  SyndromeAssessment,
}

/**
 * 体质档案：整包塞进 `/chat` 的 `payload`，透传给各 Sub-Agent。
 *
 * 字段与「创建档案」表单一一对应，只有四项：**姓名（选填）、出生日期、
 * 常住地、既往病史**。`age` 由出生日期派生，不需要用户填写。
 */
export interface PatientProfile {
  /** 姓名 / 称呼，选填。仅用于展示与归档，不参与推理。 */
  name?: string
  /** 出生日期，`YYYY-MM-DD`（Taro Date Picker 的取值格式） */
  birth_date?: string
  /** 常住地：用于地域性体质与发病倾向判断 */
  region?: string
  /** 既往病史：慢病、过敏史、手术史等自由文本 */
  history?: string
  /** 周岁，由 `birth_date` 派生（见 `utils/profile.ts#calcAge`） */
  age?: number
  /**
   * 性别：精简档案后**不再采集**，固定传 '未知'。
   *
   * 字段本身不能删——harness 的问诊步会读它（`Gender::from_payload`）
   * 过滤人群专属问诊条目；取值落到 '未知' 时后端按「不排除」处理，
   * 即宁可多问也不漏妇科鉴别线索。
   */
  gender: string
}

/**
 * 家庭成员档案。
 *
 * harness **没有**家庭/成员 API，本结构**只存在前端本地存储**（Taro Storage），
 * 用途是在发起问诊时预填体质档案。不要当作后端实体使用。
 */
export interface Member {
  id: string
  name: string
  relation: string
  patient: PatientProfile
  /**
   * 旧版遗留的「备注」字段。改版后既往病史收进 `patient.history`，
   * 这里保留只为兼容已经存到本机的老档案（读取时会被并入 history）。
   */
  note?: string
}
