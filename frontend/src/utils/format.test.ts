import { describe, it, expect } from 'vitest'
import {
  confidencePercent, categoryClass, isReferred, isFinished, TREATMENT_CATEGORY_ORDER,
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

describe('status predicates', () => {
  it('detects referred / finished', () => {
    expect(isReferred({ status: 'referred' } as any)).toBe(true)
    expect(isReferred({ status: 'finished' } as any)).toBe(false)
    expect(isFinished({ status: 'finished' } as any)).toBe(true)
  })
})

describe('TREATMENT_CATEGORY_ORDER', () => {
  it('covers the six categories', () => {
    expect(TREATMENT_CATEGORY_ORDER).toEqual([
      '中药方剂', '针灸推拿', '外治法', '西医检查', '生活调护', '膳食',
    ])
  })
})
