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

/** 体质档案：整包塞进 `/chat` 的 `payload`，透传给各 Sub-Agent。 */
export interface PatientProfile {
  region?: string
  height_cm?: number
  weight_kg?: number
  age?: number
  gender: string
  /** 静息心率（次/分），切诊可用 */
  heart_rate?: number
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
  note: string
}
