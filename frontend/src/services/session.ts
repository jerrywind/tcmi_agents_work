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
let profile: PatientProfile | null = null
let payload: Record<string, any> = {}
let result: DiagnosisResult | null = null
let round = 1

/** 开启一次新问诊：重置历史，并写入体质档案作为 payload。 */
export function startSession(p: PatientProfile): void {
  profile = p
  payload = { ...p }
  messages = []
  result = null
  round = 1
}

/**
 * 本次问诊用的体质档案。
 *
 * 档案页与问诊页是**两个页面**：档案页只收档案，病情自述在问诊页填。
 * 问诊页要用档案里的既往病史拼首轮消息，总得有个地方把它带过来。
 */
export function getProfile(): PatientProfile | null {
  return profile
}

export function getPayload(): Record<string, any> {
  // `round` 必须带上：反馈式辨证据此判断已追问了几轮，达到上限会强制放行。
  // 少了它，后端会以为永远是第一轮，于是永远不触发兜底——
  // 覆盖率始终不达标时，用户就被卡在无限追问里。
  return { ...payload, round }
}

/**
 * 用户补充信息后调一次：轮次 +1。
 *
 * 与 `pushMessage` 分开：`pushMessage` 也用于首次主诉（那时还是第 1 轮），
 * 只有「在已有结论之上继续补充」才推进轮次。
 */
export function advanceRound(): void {
  round += 1
}

export function getRound(): number {
  return round
}

export function getMessages(): HarnessMessage[] {
  return messages
}

/** 追加一条对话（user 的追问，或 user 的初始主诉）。 */
export function pushMessage(m: HarnessMessage): void {
  messages = [...messages, m]
}

/**
 * 清空对话历史。
 *
 * 首诊失败时要用：主诉在 `chat` 之前就已经 push 进去了，
 * 不撤回的话用户点一次「重试」就多发一遍，模型看到两条一模一样的主诉。
 */
export function resetMessages(): void {
  messages = []
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
  profile = null
  payload = {}
  result = null
  round = 1
}
