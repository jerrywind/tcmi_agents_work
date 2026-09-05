import type { DifferentiationStructured } from '../types'

/** 未定证时「还差哪些表现就能定证」的一条提示 */
export interface NearHint {
  slug: string
  name: string
  /** 该候选还缺哪些主症（缺这些就不满足「主症必备」） */
  missing: string[]
}

/**
 * 从结构化辨证里取出「最接近但未达标」的候选各缺哪条主症（H3 / I3）。
 *
 * **只在没有主证时才有意义**：已经定证了再列「还缺什么」是自相矛盾的。
 *
 * 这些条目是规则层确定性算出来的（`assess()` 逐条比对主症表得出），
 * 不是模型生成的内容，因此可以放心直接展示给用户——
 * 只说一句「未匹配到明确证候」用户无从行动，列出来他才知道该补充什么。
 *
 * @param diff 结构化辨证结论（`/chat` 响应的 `structured.differentiation`）
 * @param limit 最多取几条；候选再多，一次给人看三条也够了
 */
export function nearHints(
  diff: DifferentiationStructured | undefined | null,
  limit = 3,
): NearHint[] {
  if (!diff || diff.primary) return []
  return (diff.near ?? [])
    .map(n => ({ slug: n.slug, name: n.name, missing: n.missing_key_symptoms ?? [] }))
    .filter(n => n.missing.length > 0)
    .slice(0, limit)
}
