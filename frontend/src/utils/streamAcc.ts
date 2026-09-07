import { EVENTS } from '../services/stream'
import type { PlanStep, StepState, StreamEvent } from '../services/stream'

/**
 * 流式问诊的累加器：`streamChat` 的每一帧往里填一点。
 *
 * ## 为什么要从页面里抽出来
 *
 * 这段逻辑决定了「用户看到什么」——半截正文怎么拼、重试后要不要丢弃、
 * 拦截横幅何时出现。它原先写在 consult 页的 `onEvent` 里，
 * 而**组件单测为 0**，等于只能靠会 skip 的契约测试（I4）来守，
 * 也就是实际上没人守。抽成纯函数后可以直接断言（见 `streamAcc.test.ts`），
 * 与 I3 抽 `nearHints()` 是同一个理由。
 */

export interface StreamAcc {
  /** `hello` 下发的步骤计划 */
  plan: PlanStep[]
  /**
   * `hello` 下发的「系统读到的表现」：规则抽取，零 LLM 成本。
   *
   * 让用户**在 LLM 开跑之前**就能核对——读错了现在纠正只花 1 秒，
   * 等 200 秒看到结论才发现，整轮就白费了。
   */
  understood: string[]
  /** index -> 状态 */
  states: Record<number, StepState>
  /** index -> 已完成的步骤正文 */
  texts: Record<number, { capability: string; zh: string; text: string }>
  /** index -> 生成中的正文（token 增量拼接，步骤完成后转入 `texts`） */
  partial: Record<number, string>
  /** index -> 该步的重试进度（LLM 调用失败重试时才有；步骤完成/失败后清除） */
  retries: Record<number, { attempt: number; maxRetries: number; error: string }>
  /** capability -> 结构化输出（目前只有 differentiation） */
  structured: Record<string, any>
  /**
   * 上一轮各步正文（capability -> text），**仅作显示占位**。
   *
   * 追问时后端会把 10 步全部重跑，若前端清空再等，屏幕会先变空白再慢慢长出来。
   * 上一轮内容虽然可能已被新信息推翻，但标着「上一轮」先摆着，
   * 比一片空白更能说明「系统在算」，也不会被误读成新结论。
   * 新的 `step_done` 一到就按 index 覆盖，不存在「旧的没被替换」的可能。
   */
  keep: Record<string, string>
  failures: { capability: string; error: string }[]
  blocked: { slug: string; label: string; severity: string; advice: string } | null
  skipped: { capability: string; reason: string }[]
  loop: any
  confidence: { low: boolean; note: string | null } | null
  summary: string
  disclaimer: string
  reportId: string | null
  /** 是否已收到过至少一个 `step_done`——决定失败时能不能安全回退到非流式 */
  gotStep: boolean
}

export function emptyAcc(): StreamAcc {
  return {
    plan: [], understood: [], states: {}, texts: {}, partial: {}, structured: {}, keep: {},
    retries: {},
    failures: [],
    blocked: null, skipped: [], loop: null, confidence: null,
    summary: '', disclaimer: '', reportId: null, gotStep: false,
  }
}

/**
 * 把一帧事件并入累加器（原地修改并返回）。
 *
 * 未知事件一律忽略：后端加新事件时老前端不该因为不认识就崩，
 * 这是流式协议能两端独立演进的前提。
 */
export function applyStreamEvent(acc: StreamAcc, e: StreamEvent): StreamAcc {
  const d = e.data || {}
  switch (e.event) {
    case EVENTS.HELLO:
      acc.plan = d.plan || []
      acc.understood = d.understood || []
      break
    case EVENTS.STEP_START:
      if (acc.states[d.index] !== 'done') acc.states[d.index] = 'running'
      break
    // token 增量：模型还在写，先把已生成的部分显示出来。
    // 用户不必等这一步结束——这是「减少等待」里最实在的一刀。
    case EVENTS.STEP_DELTA:
      acc.partial[d.index] = (acc.partial[d.index] || '') + (d.delta || '')
      if (acc.states[d.index] !== 'done') acc.states[d.index] = 'running'
      break
    // 重试：先丢掉已累积的半截正文——后端会把正文从头再推一遍，
    // 累加就变成两份（L5 实测：一次开方步 361s 全耗在重试上）。
    // 同时把该步标回 running，让用户看到「还在试」而不是干等。
    case EVENTS.STEP_RETRY:
      delete acc.partial[d.index]
      acc.states[d.index] = 'running'
      acc.retries[d.index] = {
        attempt: d.attempt,
        maxRetries: d.max_retries,
        error: d.error,
      }
      break
    case EVENTS.STEP_DONE:
      acc.gotStep = true
      acc.states[d.index] = 'done'
      acc.texts[d.index] = { capability: d.capability, zh: d.zh, text: d.text }
      delete acc.partial[d.index]
      delete acc.retries[d.index]
      if (d.structured) acc.structured[d.capability] = d.structured
      break
    case EVENTS.STEP_FAIL:
      acc.states[d.index] = 'failed'
      delete acc.retries[d.index]
      acc.failures.push({ capability: d.capability, error: d.error })
      break
    // 预检与确认两条路径数据一致，后到的覆盖先到的，视觉上不闪烁
    case EVENTS.RED_FLAG:
    case EVENTS.BLOCKED:
      acc.blocked = {
        slug: d.slug, label: d.label, severity: d.severity, advice: d.advice,
      }
      break
    case EVENTS.SKIPPED:
      acc.skipped = (d.capabilities || []).map((c: any) => ({
        capability: c.capability, reason: d.reason,
      }))
      break
    case EVENTS.LOOP:
      acc.loop = d
      break
    case EVENTS.CONFIDENCE:
      acc.confidence = d
      break
    case EVENTS.SUMMARY:
      acc.summary = d.text || ''
      break
    case EVENTS.DONE:
      acc.reportId = d.report_id ?? null
      acc.disclaimer = d.disclaimer || ''
      break
    default:
      break
  }
  return acc
}
