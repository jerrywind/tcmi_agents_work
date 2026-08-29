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

/** 失败的步骤（`/chat` 部分失败降级时才有） */
export interface DiagnosisFailure {
  capability: HarnessCapability
  error: string
}

/** 因安全门拦截而未执行的步骤 */
export interface DiagnosisSkipped {
  capability: HarnessCapability
  reason: string
}

/** 单步埋点：耗时 / token / 模型 / 工具调用 / 错误 */
export interface StepTrace {
  capability: HarnessCapability
  name: string
  duration_ms: number
  model: string
  llm_calls: number
  llm_attempts: number
  llm_duration_ms: number
  prompt_tokens?: number | null
  completion_tokens?: number | null
  total_tokens?: number | null
  tool_calls: string[]
  error?: string | null
}

/**
 * 单个证候的结构化评估（T4.1）。
 *
 * 由 harness 的辨证步骤随 `structured` 返回：**确定性**产出，不随 LLM 波动，
 * 因此前端可以直接按字段渲染，无需从 Markdown 正文里反解析。
 */
export interface SyndromeAssessment {
  slug: string
  name: string
  /** 置信度 0~1 */
  confidence: number
  /** 支持证据：命中的症状 / 舌象 / 脉象 / 关键词证据标签 */
  supporting: string[]
  /** 矛盾证据：语料中出现了与命中表现相反的表现 */
  conflicting: string[]
  pathogenesis?: string | null
}

/** 结构化辨证结论：主证 + 兼证（T4.2）+ 传变提示 */
export interface DifferentiationStructured {
  /** 证据不足时为 null */
  primary: SyndromeAssessment | null
  concurrent: SyndromeAssessment[]
  transformations: string[]
}

/** POST /chat 的响应 */
export interface DiagnosisResult {
  steps: DiagnosisStep[]
  summary: string
  /** 失败的步骤；为空表示全部成功 */
  failures?: DiagnosisFailure[]
  /** 是否存在失败步骤（结果不完整） */
  partial?: boolean
  /** 是否被安全门拦截（命中 high/critical 红旗） */
  blocked?: boolean
  /** 拦截原因：`标签·级别：建议` */
  block_reason?: string | null
  /** 因拦截而未执行的步骤 */
  skipped?: DiagnosisSkipped[]
  /** 每步埋点 */
  trace?: StepTrace[]
  /**
   * 各步骤的结构化输出，按 capability 键（T4.1）。
   * 目前只有 `differentiation` 会产出；无结构化结果的步骤不出现在该对象里。
   */
  structured?: {
    differentiation?: DifferentiationStructured
  } | null
  /**
   * 归档报告 id（T5.1）：服务端配置了 `store_dir` 才有值，
   * 未启用持久化时为 `null`。可用 `GET /reports/:id` 回查。
   */
  report_id?: string | null
  /**
   * 服务端下发的免责声明（T5.4 合规）。
   * 前端**必须**展示，且不得由用户关闭——AI 健康建议被误当诊断是最需防的风险。
   */
  disclaimer?: string
}

/** 归档报告的列表项（`GET /reports`） */
export interface ReportMeta {
  id: string
  created_at: string | null
  /** 该次问诊是否有步骤失败（结果不完整） */
  partial: boolean
  /** 是否被安全门拦截 */
  blocked: boolean
  steps: number
  /** 主证名（无结构化结论时为 null） */
  primary_syndrome: string | null
}

export interface ReportsResult {
  reports: ReportMeta[]
  /** 服务端是否启用了报告持久化；false 时 reports 恒为空 */
  enabled: boolean
  /** 未启用时的说明（不是错误，故服务端用 hint 而非 error 字段） */
  hint?: string
  error?: string
}

/** 归档报告详情（`GET /reports/:id`），即一次 `/chat` 的完整快照 */
export interface StoredReport {
  id: string
  created_at: string
  /** 存档时已脱敏的问诊输入 */
  messages: HarnessMessage[]
  payload: Record<string, any>
  result: DiagnosisResult
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
): Promise<{
  capability: HarnessCapability
  content: string
  /** 结构化输出（T4.1）；该步骤无结构化结果时为 null */
  structured?: Record<string, any> | null
}> {
  return harnessRequest<{
    capability: HarnessCapability
    content: string
    structured?: Record<string, any> | null
  }>(
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

/** 列出已归档报告（T5.1）；服务端未启用持久化时返回 `enabled: false` */
export function listReports(limit?: number): Promise<ReportsResult> {
  const q = limit && limit > 0 ? `?limit=${limit}` : ''
  return harnessRequest<ReportsResult>('GET', `/reports${q}`)
}

/** 按 id 回查一份归档报告（T5.1） */
export function getReport(id: string): Promise<StoredReport> {
  return harnessRequest<StoredReport>('GET', `/reports/${encodeURIComponent(id)}`)
}

/** `/mcp` 返回的 MCP 工具定义（T4.5） */
export interface McpTool {
  name: string
  description: string
  inputSchema: Record<string, any>
}

/** JSON-RPC 2.0 响应信封 */
export interface McpRpcResponse {
  jsonrpc: string
  id: number | string | null
  result?: any
  error?: { code: number; message: string }
}

/**
 * 调用 harness 内置的 MCP Server 端点（T4.5）。
 *
 * 用于让外部 MCP 客户端（Claude Desktop / Cursor 等）通过标准协议调用本系统能力；
 * 前端自身不走这条链路（有更直接的 `/chat`、`/agents`）。
 *
 * 返回完整信封（含 `result` / `error`），由调用方判定成败：
 * MCP 约定「工具执行失败」也回 200 + `isError`，与 harness 其它端点的
 * `{"error": ...}` 习惯不同，故这里不做统一抛错。
 */
export function mcpRpc(
  method: string,
  params: Record<string, any> = {},
  id: number | string = 1,
): Promise<McpRpcResponse> {
  return harnessRequest<McpRpcResponse>('POST', '/mcp', { jsonrpc: '2.0', id, method, params })
}

/** 列出 MCP 对外暴露的工具（tools/list） */
export function listMcpTools(): Promise<McpRpcResponse> {
  return mcpRpc('tools/list')
}
