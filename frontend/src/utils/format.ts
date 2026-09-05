/** 置信度（0~1）转百分比字符串，缺失按 0 处理。 */
export function confidencePercent(confidence: number | undefined | null): string {
  return `${Math.round((confidence ?? 0) * 100)}%`
}

/**
 * 把「主证 + 兼证」拼成一行摘要（T4.2）。
 *
 * 兼证是**并存**关系而非备选，因此用「+」连接，并各带置信度：
 * `风寒感冒 60% + 肝郁气滞 40%`。
 *
 * 入参刻意用最小结构（只取 name / confidence），
 * 便于直接喂 `SyndromeAssessment`，也便于单测不依赖后端类型。
 */
export function syndromeSummary(
  primary: { name: string; confidence?: number | null } | null | undefined,
  concurrent: Array<{ name: string; confidence?: number | null }> = [],
): string {
  if (!primary) return ''
  const parts = [
    `${primary.name} ${confidencePercent(primary.confidence)}`,
    ...concurrent.map(c => `${c.name} ${confidencePercent(c.confidence)}`),
  ]
  return parts.join(' + ')
}

/** 诊疗方案类别 -> 色标 class（与 index.scss 中 .cat-* 对应）。 */
export function categoryClass(category: string): string {
  return `cat-${category}`
}

/** 诊疗方案类别展示顺序。 */
export const TREATMENT_CATEGORY_ORDER: string[] = [
  '中药方剂', '针灸推拿', '外治法', '西医检查', '生活调护', '膳食',
]

/**
 * 截断长文本，超出部分以省略号结尾。
 * 报告里 LLM 输出可能很长，列表场景需要截断展示。
 */
export function truncate(text: string, max = 120): string {
  if (!text) return ''
  return text.length > max ? `${text.slice(0, max)}…` : text
}

/**
 * 去掉 Markdown 强调标记，只留纯文本。
 *
 * 后端生成的 `confidence_note` 带 `**粗体**`：它会被**同时**拼进报告正文
 * （那里按 Markdown 渲染，需要强调）和随响应下发给前端做提示条
 * （这里是纯文本，星号会原样露出来，读起来像乱码）。
 * 真机验证时提示条上就显示着「本次**未匹配到明确证候**」。
 *
 * 只处理强调标记：不打算在提示条里支持一整套 Markdown，
 * 剥掉 `**` 即可，其余字符原样保留。
 */
export function stripMarkdown(text: string): string {
  if (!text) return ''
  return text.replace(/\*\*(.+?)\*\*/g, '$1')
}
