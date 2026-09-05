import Taro from '@tarojs/taro'

/**
 * harness（Rust 后端）契约客户端。
 *
 * harness 是**无状态**服务，不保存问诊会话：没有 `cons_xxx` 会话 id，
 * 也没有 start/answer/report 等会话端点。
 * 多轮问诊由调用方（前端）维护 `messages` 数组，每次带上完整对话历史。
 *
 * 端点：GET /health、GET|POST /agents、POST /chat、GET|POST /skills、POST /reload
 *
 * 说明：面向已归档 Python backend 的旧契约 `api.ts` 已**删除**，
 * 本模块是唯一的后端访问层（多轮状态见 `services/session.ts`）。
 */

/**
 * 读取构建期环境变量。
 *
 * **浏览器里没有 `process`**。模块顶层直接写 `process.env.X` 会抛
 * ReferenceError，整个模块加载失败 → 页面白屏，只剩一个导航栏。
 * H5 端曾因此完全不可用，而编译能过、单测也全绿——
 * 只有真机打开页面才会暴露。webpack 会把 `process.env.TARO_ENV`
 * 做字面替换，未被替换时则落到这里的 `typeof` 兜底。
 */
function envVar(key: string): string | undefined {
  if (typeof process === 'undefined' || !process.env) return undefined
  return (process.env as Record<string, string | undefined>)[key]
}

/**
 * 是否为 H5 端。
 *
 * 优先用 Taro 的构建期常量（webpack 会把 `process.env.TARO_ENV` 替换成字面量）；
 * 未被替换时（浏览器里没有 `process`）退化为「有 `window` 就是 H5」。
 * 三种环境因此都能区分开：
 * - H5 浏览器：无 `process`、有 `window` → true
 * - 小程序：两者都没有 → false
 * - Node（单测）：有 `process` → 短路为 false，与既有测试预期一致
 *
 * 不用 `Taro.getEnv()`：它在单测环境里并不存在（会抛 `getEnv is not a function`）。
 */
const IS_H5 =
  envVar('TARO_ENV') === 'h5' ||
  (typeof process === 'undefined' && typeof window !== 'undefined')

export const HARNESS_BASE_URL = IS_H5
  ? ''
  : envVar('VITE_API_BASE') || 'http://127.0.0.1:8011'

// 经 nginx / devServer 代理时端点带 /api 前缀（由网关剥离后转发到 harness）；
// 直连 harness 时无前缀（小程序直连场景）。
export const HARNESS_API_PREFIX = envVar('VITE_API_PREFIX') ?? (IS_H5 ? '/api' : '')

/**
 * `/chat` 的请求超时（毫秒）。
 *
 * 一次 `/chat` 是「跑完全部步骤再一次性返回」，标准档 10 步实测 200–530 秒。
 * 超时必须大于这个量级，否则前端会在后端还算着的时候放弃。
 */
export const REQUEST_TIMEOUT_MS = 600_000

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
  /**
   * 是否满足「主症必备」（H3）：至少命中一条主症。
   * 只有合格的候选才能成为主证或兼证。
   */
  qualified?: boolean
  /** 未命中的主症：说明「差在哪」，未匹配时可据此提示患者补充（H3） */
  missing_key_symptoms?: string[]
}

/** 结构化辨证结论：主证 + 兼证（T4.2）+ 传变提示 */
export interface DifferentiationStructured {
  /** 证据不足（未满足主症必备或孤证不立）时为 null */
  primary: SyndromeAssessment | null
  concurrent: SyndromeAssessment[]
  transformations: string[]
  /** `primary !== null` 的镜像，便于直读 */
  matched?: boolean
  /** 全部有命中的候选（按证据量降序，含未合格的） */
  ranked?: SyndromeAssessment[]
  /**
   * 未匹配时「最接近但未达标」的候选（H3）。
   * 展示它比只写「未匹配」更有用：用户知道还差哪一项才能定证。
   */
  near?: SyndromeAssessment[]
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
  /**
   * `awaiting_input` = 信息不足，流程停在辨证等补充（**此时没有治疗建议**）；
   * `completed` = 已跑完。
   *
   * 前端不判断这个字段的话，用户会看到一个「缺了治疗建议」的半截报告，
   * 却不知道该继续补充什么。
   */
  status?: 'awaiting_input' | 'completed'
  /**
   * 反馈式辨证状态（T3.x）：调用方未在 `payload.syndrome` 给定证候时有值。
   * 前端据此提示「还缺什么」，并在补充后带上递增的 `round` 重新请求。
   */
  loop?: DiagnosisLoop
  /**
   * 结论可信度不足（H4/H5）：未匹配到证候 / 置信度未达锁定门槛 /
   * 达到最大追问轮次被强制放行，三者任一成立即为 `true`。
   *
   * 此前这三种情形在响应里毫无痕迹，读报告的人无从分辨。
   * 与 `disclaimer` 同属**必须展示**的内容——区别在于 disclaimer 是固定
   * 免责声明，这条是本次结论特有的质量信号。
   */
  low_confidence?: boolean
  /** 可直接展示给用户的中文说明（low_confidence 为 true 时非空） */
  confidence_note?: string | null
}

/** 待补充的问诊条目（由 `questions.yaml` 等规则确定性产出，不是模型编的） */
export interface PendingQuestion {
  slug: string
  text: string
  reason?: string
  source?: string
  agent?: string
  priority?: number
}

/** 反馈式辨证的当前轮状态 */
export interface DiagnosisLoop {
  round: number
  converged: boolean
  /** 达到最大轮次被强制放行（保证最终一定有结论，不把用户卡在无限追问里） */
  forced: boolean
  confidence: number
  margin: number
  /** 必采信息覆盖率 0~1 */
  coverage: number
  primary?: string | null
  pending_questions?: PendingQuestion[]
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
      // 一次 `/chat` 会把 routing 里**全部步骤串行跑完**，每步一次 LLM 调用：
      // 标准档 10 步实测 200–530 秒（模型与语料不同波动很大）。
      // 此前这里是 120 秒，多数问诊还没跑完就被前端掐断，
      // 用户看到的是「网络异常」——而后端其实还在正常算。
      //
      // ⚠️ 小程序端受平台限制（wx.request 超时上限约 60 秒），
      // 设得再大也不生效：**小程序端用不了完整串行流程**，
      // 需要改成分步请求或轮询任务结果，那是架构改造，另立条目。
      timeout: REQUEST_TIMEOUT_MS,
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

/** `/health` 响应（T7.5 起为 JSON，此前是纯文本 'ok'） */
export interface HealthStatus {
  /** 进程是否存活。RAG 不可用**不影响**这里：那是「没查到典籍」，不是服务挂了 */
  status: string
  rag?: {
    configured: boolean
    /** 最近一次探测是否成功；null = 还没探测过或未配置 */
    reachable?: boolean | null
    endpoint?: string
    last_error?: string
    since_last_ok_secs?: number
  }
}

/** 健康检查 */
export function health(): Promise<HealthStatus> {
  return harnessRequest<HealthStatus>('GET', '/health')
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
