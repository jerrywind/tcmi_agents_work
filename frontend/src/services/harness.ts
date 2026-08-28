import Taro from '@tarojs/taro'

/**
 * harness（Rust 后端）契约客户端。
 *
 * 与旧 `api.ts` 的关键差异：harness 是**无状态**服务，不保存问诊会话，
 * 没有 `cons_xxx` 会话 id，也没有 start/answer/report 等会话端点。
 * 多轮问诊由调用方（前端）维护 `messages` 数组，每次带上完整对话历史。
 *
 * 端点：GET /health、GET|POST /agents、POST /chat、GET|POST /skills、POST /reload
 *
 * 说明：本模块与旧 `api.ts` 并存，便于渐进迁移；旧模块面向已归档的 Python backend。
 */

// H5 走 devServer 代理（config/dev.ts 已把 /api 转发到 harness:8011 并剥离前缀）；
// 小程序/RN 直连后端地址（可用 VITE_API_BASE 覆盖）。
export const HARNESS_BASE_URL =
  process.env.TARO_ENV === 'h5'
    ? ''
    : process.env.VITE_API_BASE || 'http://127.0.0.1:8011'

// 经 nginx / devServer 代理时端点带 /api 前缀（由网关剥离后转发到 harness）；
// 直连 harness 时无前缀（小程序直连场景）。
export const HARNESS_API_PREFIX =
  process.env.VITE_API_PREFIX ?? (process.env.TARO_ENV === 'h5' ? '/api' : '')

/** 对话消息 */
export interface HarnessMessage {
  role: 'user' | 'assistant' | 'system'
  content: string
}

/** harness 的 capability（无前缀 slug） */
export type HarnessCapability =
  | 'inspection'      // 望诊
  | 'listening'       // 闻诊
  | 'inquiry'         // 问诊
  | 'palpation'       // 切诊
  | 'differentiation' // 辨证
  | 'safety'          // 安全门
  | 'treatment'       // 治疗

export const CAPABILITY_ZH: Record<HarnessCapability, string> = {
  inspection: '望诊',
  listening: '闻诊',
  inquiry: '问诊',
  palpation: '切诊',
  differentiation: '辨证',
  safety: '安全门',
  treatment: '治疗',
}

/** 诊断流程中单步产出 */
export interface DiagnosisStep {
  capability: HarnessCapability
  text: string
}

/** POST /chat 的响应 */
export interface DiagnosisResult {
  steps: DiagnosisStep[]
  summary: string
}

/** GET /agents 的响应 */
export interface AgentsResult {
  capabilities: HarnessCapability[]
  names: string[]
}

/** GET /skills 的单条技能 */
export interface HarnessSkill {
  name: string
  description: string
  owner: string
}

export interface SkillsResult {
  skills: HarnessSkill[]
}

async function harnessRequest<T>(
  method: 'GET' | 'POST',
  url: string,
  data?: any,
): Promise<T> {
  let res
  try {
    res = await Taro.request({
      url: `${HARNESS_BASE_URL}${HARNESS_API_PREFIX}${url}`,
      method,
      data,
      header: { 'Content-Type': 'application/json' },
      timeout: 120000,
    })
  } catch {
    throw new Error('网络异常')
  }
  // harness 的错误也以 200 + {"error": "..."} 返回，需额外识别
  if (res.statusCode >= 400) {
    const detail = (res.data && (res.data as any).detail) || `HTTP ${res.statusCode}`
    throw new Error(String(detail))
  }
  const body = res.data as any
  if (body && typeof body === 'object' && typeof body.error === 'string') {
    throw new Error(body.error)
  }
  return body as T
}

/** 健康检查：返回 'ok' */
export function health(): Promise<string> {
  return harnessRequest<string>('GET', '/health')
}

/** 列出已注册的 Sub-Agent 能力 */
export function listAgents(): Promise<AgentsResult> {
  return harnessRequest<AgentsResult>('GET', '/agents')
}

/**
 * 完整诊断流程：按 resources/routing.yaml 的顺序依次调用各 Sub-Agent。
 * 调用方需自行维护多轮对话历史（harness 无会话存储）。
 */
export function chat(
  messages: HarnessMessage[],
  payload: Record<string, any> = {},
): Promise<DiagnosisResult> {
  return harnessRequest<DiagnosisResult>('POST', '/chat', { messages, payload })
}

/** 单步调用某个 Sub-Agent */
export function runAgent(
  capability: HarnessCapability,
  messages: HarnessMessage[],
  payload: Record<string, any> = {},
): Promise<{ capability: HarnessCapability; content: string }> {
  return harnessRequest<{ capability: HarnessCapability; content: string }>(
    'POST',
    '/agents',
    { capability, messages, payload },
  )
}

/** 列出可用技能（harness 内置注册，不支持运行时装载/卸载） */
export function listSkills(): Promise<SkillsResult> {
  return harnessRequest<SkillsResult>('GET', '/skills')
}

/** 执行某个技能 */
export function callSkill(
  name: string,
  arguments_: Record<string, any> = {},
): Promise<{ result: any }> {
  return harnessRequest<{ result: any }>('POST', '/skills', {
    name,
    arguments: arguments_,
  })
}

/** 热重载 YAML 资源（需服务端 hot_reload: true） */
export function reloadResources(): Promise<{ ok: boolean }> {
  return harnessRequest<{ ok: boolean }>('POST', '/reload')
}
