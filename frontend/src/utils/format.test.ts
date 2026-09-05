import { describe, it, expect } from 'vitest'
import {
  confidencePercent, categoryClass, truncate, TREATMENT_CATEGORY_ORDER,
  syndromeSummary, stripMarkdown,
} from './format'

describe('confidencePercent', () => {
  it('converts 0~1 to percent', () => {
    expect(confidencePercent(0.85)).toBe('85%')
    expect(confidencePercent(1)).toBe('100%')
  })
  it('handles missing value as 0', () => {
    expect(confidencePercent(undefined)).toBe('0%')
    expect(confidencePercent(null)).toBe('0%')
  })
  it('rounds correctly', () => {
    expect(confidencePercent(0.333)).toBe('33%')
  })
})

describe('categoryClass', () => {
  it('prefixes cat-', () => {
    expect(categoryClass('中药方剂')).toBe('cat-中药方剂')
  })
})

describe('truncate', () => {
  it('keeps short text as-is', () => {
    expect(truncate('短文本')).toBe('短文本')
  })
  it('truncates long text with ellipsis', () => {
    expect(truncate('a'.repeat(200), 10)).toBe('aaaaaaaaaa…')
  })
  it('handles empty input', () => {
    expect(truncate('')).toBe('')
  })
})

// T4.2：兼证要在报告里与主证并列呈现，不能被当成备选项折叠掉
describe('syndromeSummary', () => {
  it('renders primary with its confidence', () => {
    expect(syndromeSummary({ name: '风寒感冒', confidence: 0.6 })).toBe('风寒感冒 60%')
  })

  it('joins concurrent syndromes with plus sign', () => {
    const r = syndromeSummary(
      { name: '风寒感冒', confidence: 0.6 },
      [{ name: '肝郁气滞', confidence: 0.4 }],
    )
    expect(r).toBe('风寒感冒 60% + 肝郁气滞 40%')
  })

  it('returns empty string when there is no primary syndrome', () => {
    expect(syndromeSummary(null)).toBe('')
    expect(syndromeSummary(undefined, [{ name: '肝郁气滞', confidence: 0.4 }])).toBe('')
  })

  it('treats missing confidence as 0', () => {
    expect(syndromeSummary({ name: '风寒感冒' })).toBe('风寒感冒 0%')
  })
})

describe('TREATMENT_CATEGORY_ORDER', () => {
  it('covers the six categories', () => {
    expect(TREATMENT_CATEGORY_ORDER).toEqual([
      '中药方剂', '针灸推拿', '外治法', '西医检查', '生活调护', '膳食',
    ])
  })
})

// 后端 confidence_note 带 Markdown 强调：正文里按 Markdown 渲染需要它，
// 但提示条是纯文本，星号会原样露出来（真机验证时看到过）
describe('stripMarkdown', () => {
  it('removes bold markers but keeps the text', () => {
    expect(stripMarkdown('本次**未匹配到明确证候**：请线下就诊'))
      .toBe('本次未匹配到明确证候：请线下就诊')
  })

  it('handles multiple occurrences', () => {
    expect(stripMarkdown('**甲**与**乙**')).toBe('甲与乙')
  })

  it('leaves plain text untouched', () => {
    expect(stripMarkdown('已达最大追问轮次（3 轮）')).toBe('已达最大追问轮次（3 轮）')
  })

  it('leaves single asterisks alone: only ** is emphasis', () => {
    expect(stripMarkdown('a*b*c')).toBe('a*b*c')
  })

  it('handles empty input', () => {
    expect(stripMarkdown('')).toBe('')
  })
})
