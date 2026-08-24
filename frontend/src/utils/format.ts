import type { ConsultState } from '../types'

/** 置信度（0~1）转百分比字符串，缺失按 0 处理。 */
export function confidencePercent(confidence: number | undefined | null): string {
  return `${Math.round((confidence ?? 0) * 100)}%`
}

/** 诊疗方案类别 -> 色标 class（与 index.scss 中 .cat-* 对应）。 */
export function categoryClass(category: string): string {
  return `cat-${category}`
}

/** 诊疗方案类别展示顺序。 */
export const TREATMENT_CATEGORY_ORDER: string[] = [
  '中药方剂', '针灸推拿', '外治法', '西医检查', '生活调护', '膳食',
]

/** 是否已转急诊/就医。 */
export function isReferred(state: ConsultState): boolean {
  return state.status === 'referred'
}

/** 是否已生成最终报告。 */
export function isFinished(state: ConsultState): boolean {
  return state.status === 'finished'
}
