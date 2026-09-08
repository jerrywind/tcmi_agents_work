import Taro from '@tarojs/taro'
import type { DiagnosisResult, HarnessMessage } from './harness'
import type { PatientProfile } from '../types'

/**
 * 前端会话容器。
 *
 * harness 是**无状态**服务：不保存会话、没有会话 id，
 * 多轮问诊必须由调用方维护完整的 `messages` 数组。
 * 本模块即在 `index → consult → report` 之间共享这份状态。
 *
 * ## 本地快照（为什么现在要落 Storage）
 *
 * 一次标准档 `/chat` 要跑 200–530 秒。若刷新/切后台/被系统回收后全部状态
 * 丢失，用户就白等一整轮——这是「用户侧最贵的一次等待」。这里把会话进度
 * 落到本地 Storage：
 *
 * - **已有完整结论**（`result` 非空）：连历史一起恢复。刷新后仍能看到上一份
 *   报告，并能继续追问，相当于刷新没有发生。
 * - **进行中被打断**（无 `result`）：harness 无状态、无法续跑，若把半截
 *   messages 留下，用户重新填主诉会 append 出「两遍主诉」。所以只保留档案，
 *   让首诊表单干净可用。
 *
 * 图片 data URL（舌苔/手相，单张可达数 MB）**不落盘**：超出 localStorage
 * 配额只会让快照变成例外。刷新后图片槽为空，重新上传即可（本就选填）。
 */
let messages: HarnessMessage[] = []
let profile: PatientProfile | null = null
let payload: Record<string, any> = {}
let result: DiagnosisResult | null = null
let round = 1

// ---------------------------------------------------------------- 本地快照

export const SESSION_KEY = 'tcm_consult_session_v1'

interface PersistedSession {
  profile: PatientProfile | null
  payload: Record<string, any>
  round: number
  messages: HarnessMessage[]
  result: DiagnosisResult | null
}

/** hydrate 后的一次性提示（见 `consumeSessionNotice`） */
let recoveredNotice: 'restored' | 'interrupted' | null = null

/** 尽力写；失败静默——快照只是「刷新别白等」的增强，不能拖垮主流程 */
function persist(): void {
  try {
    const snap: PersistedSession = { profile, payload, round, messages, result }
    Taro.setStorageSync(SESSION_KEY, JSON.stringify(snap))
  } catch {
    // 忽略：配额满 / 隐私模式 / 序列化失败都不该中断问诊
  }
}

function clearPersisted(): void {
  try {
    Taro.removeStorageSync(SESSION_KEY)
  } catch {
    // 忽略
  }
}

/**
 * 模块加载时恢复上次会话。
 *
 * 只在 `result` 完整存在时才恢复消息与轮次；否则视为「中断会话」作废，
 * 仅回填档案与 payload（规则详见文件头注释）。
 */
function hydrate(): void {
  let raw: unknown = null
  try {
    raw = Taro.getStorageSync(SESSION_KEY)
  } catch {
    return
  }
  let s: PersistedSession | null = null
  if (typeof raw === 'string') {
    try {
      s = JSON.parse(raw) as PersistedSession
    } catch {
      return
    }
  }
  if (!s || typeof s !== 'object' || !s.profile) return
  profile = s.profile
  payload = s.payload && typeof s.payload === 'object' ? s.payload : { ...s.profile }
  if (s.result && Array.isArray(s.messages) && s.messages.length > 0) {
    result = s.result
    messages = s.messages
    round = Math.max(1, typeof s.round === 'number' ? s.round : 1)
    recoveredNotice = 'restored'
  } else {
    messages = []
    round = 1
    recoveredNotice = 'interrupted'
  }
}

hydrate()

/** 消费一次 hydrate 提示：`restored`（恢复了上次结论）或 `interrupted`（上次未完成已作废）。 */
export function consumeSessionNotice(): 'restored' | 'interrupted' | null {
  const n = recoveredNotice
  recoveredNotice = null
  return n
}

/** 开启一次新问诊：重置历史，并写入体质档案作为 payload。 */
export function startSession(p: PatientProfile): void {
  profile = p
  payload = { ...p }
  messages = []
  result = null
  round = 1
  persist()
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
  persist()
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
  persist()
}

/**
 * 清空对话历史。
 *
 * 首诊失败时要用：主诉在 `chat` 之前就已经 push 进去了，
 * 不撤回的话用户点一次「重试」就多发一遍，模型看到两条一模一样的主诉。
 */
export function resetMessages(): void {
  messages = []
  persist()
}

/** 会话快照：`pushMessage` / `advanceRound` 之前的消息数与轮次。 */
export interface SessionSnapshot {
  messages: number
  round: number
}

/**
 * 记录当前会话进度，供请求失败时回滚（配 `rollbackSession`）。
 *
 * 追问链路是「先 push 用户消息 + 推进轮次，再发 `/chat`」——
 * 请求失败时这两个副作用都已经落地了。
 */
export function snapshotSession(): SessionSnapshot {
  return { messages: messages.length, round }
}

/**
 * 回滚到快照点：只回退消息与轮次，档案和已有结论保持不动。
 *
 * 与 `resetMessages`（清空全部历史、但保留轮次）的区别就在轮次：
 * 追问失败必须把轮次也退回去——否则每失败一次就白吃一轮追问预算，
 * 后端的「达到上限强制放行」会提前触发，在用户根本没补充到信息的情况下
 * 强行给出结论。
 */
export function rollbackSession(s: SessionSnapshot): void {
  messages = messages.slice(0, Math.max(0, s.messages))
  round = Math.max(1, s.round)
  persist()
}

/**
 * 记录一次 `/chat` 的结果，并把助手输出回灌进历史，
 * 这样下一轮追问时模型能看到之前说过什么。
 */
export function setResult(r: DiagnosisResult): void {
  result = r
  messages = [...messages, { role: 'assistant', content: r.summary }]
  persist()
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
  clearPersisted()
}
