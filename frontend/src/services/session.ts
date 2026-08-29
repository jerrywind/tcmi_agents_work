import type { DiagnosisResult, HarnessMessage } from './harness'
import type { PatientProfile } from '../types'

/**
 * 前端会话容器。
 *
 * harness 是**无状态**服务：不保存会话、没有会话 id，
 * 多轮问诊必须由调用方维护完整的 `messages` 数组。
 * 本模块即在 `index → consult → report` 之间共享这份状态。
 *
 * 有意**不做持久化**：harness 侧没有会话可以恢复，
 * 落地到 Storage 反而会让用户以为刷新后能续诊。
 */
let messages: HarnessMessage[] = []
let payload: Record<string, any> = {}
let result: DiagnosisResult | null = null

/** 开启一次新问诊：重置历史，并写入体质档案作为 payload。 */
export function startSession(profile: PatientProfile): void {
  payload = { ...profile }
  messages = []
  result = null
}

export function getPayload(): Record<string, any> {
  return payload
}

export function getMessages(): HarnessMessage[] {
  return messages
}

/** 追加一条对话（user 的追问，或 user 的初始主诉）。 */
export function pushMessage(m: HarnessMessage): void {
  messages = [...messages, m]
}

/**
 * 记录一次 `/chat` 的结果，并把助手输出回灌进历史，
 * 这样下一轮追问时模型能看到之前说过什么。
 */
export function setResult(r: DiagnosisResult): void {
  result = r
  messages = [...messages, { role: 'assistant', content: r.summary }]
}

export function getResult(): DiagnosisResult | null {
  return result
}

export function clearSession(): void {
  messages = []
  payload = {}
  result = null
}
