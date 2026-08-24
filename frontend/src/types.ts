export interface QuestionOption { label: string; value: string }

export interface Question {
  id: string
  key: string
  text: string
  options: QuestionOption[]
  allow_free_text: boolean
}

export interface Hypothesis {
  name: string
  confidence: number
  supporting: string[]
  contradicting: string[]
}

export interface Message {
  id: string
  role: 'agent' | 'user' | 'system'
  type: 'text' | 'question' | 'report' | 'alert'
  content: string
  ts: number
}

export interface TreatmentPlan {
  id: string
  category: '中药方剂' | '针灸推拿' | '外治法' | '西医检查' | '生活调护' | '膳食'
  title: string
  detail?: string
  rationale?: string
  note?: string
  priority: number
  warnings?: string[]
}

export interface Report {
  syndromes: Hypothesis[]
  reasoning: string
  advice: Record<string, string>
  treatments: TreatmentPlan[]
  red_flag?: string | null
  sources?: string[]
  evolution?: string
  disclaimer: string
}

export interface CareTodo {
  id: string
  title: string
  category: string
  detail: string
  kind: 'decoct' | 'checkin' | 'appointment'
  times: string[]
  done: boolean
}

export interface FollowUp {
  id: string
  due_in_days: number
  focus: string
  done: boolean
  feedback: string
}

export interface RevisitChange {
  key: string
  before: string
  after: string
  improved: 'better' | 'worse' | 'unknown'
}

export interface RevisitCompare {
  has_baseline: boolean
  baseline_ts?: number
  revisit_ts?: number
  changes: RevisitChange[]
}

export interface RevisitImage {
  id: string
  ts: number
  path: string
  kind: string
  features: Record<string, string>
}

export interface LabIndicator {
  name: string
  value: string
  abnormal: boolean
}

export interface LabResult {
  tcm_note: string
  indicators: LabIndicator[]
  evidence_keys: string[]
}

export interface ConsultState {
  id: string
  status: 'created' | 'running' | 'waiting_answer' | 'planning' | 'treatment_qa' | 'finished' | 'referred'
  task_id?: string
  round: number
  family_id?: string
  member_id?: string
  ppg?: PpgReading | null
  question?: Question | null
  hypotheses: Hypothesis[]
  messages: Message[]
  report?: Report | null
  care_todos?: CareTodo[]
}

// ---------- PPG 脉象 ----------
export interface PpgReading {
  rate_bpm: number
  rhythm: string
  depth: string
  force: string
  shape: string
  amplitude: number
  perfusion: number
  signal_quality: number
  notes: string
  ts: number
}

export interface PatientForm {
  region?: string
  height_cm?: number
  weight_kg?: number
  age?: number
  gender: string
  name?: string
}

// ---------- 家庭 / 成员 ----------
export interface Family {
  id: string
  name: string
  owner: string
  members: Member[]
  created_at: number
}

export interface Member {
  id: string
  family_id: string
  name: string
  relation: string
  patient: PatientForm
  note: string
  created_at: number
}

export interface FamilyConsultSummary {
  id: string
  member_id: string
  status: string
  complaint: string
  ts: number
  syndromes: string[]
}

// SKILL：LLM 可调用工具集
export interface SkillTool {
  name: string
  description: string
  parameters: any
  capability: string
}

export interface Skill {
  name: string
  version: string
  description: string
  tools: SkillTool[]
}

export interface SkillsList {
  skills_dir: string
  skills: Skill[]
  tools: SkillTool[]
}
