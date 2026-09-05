import { describe, expect, it } from 'vitest'
import { nearHints } from './differentiation'
import type { DifferentiationStructured } from '../types'

/** 构造一个候选；只填本函数关心的字段 */
const cand = (slug: string, name: string, missing: string[] | undefined) => ({
  slug,
  name,
  confidence: 0.4,
  supporting: [],
  conflicting: [],
  missing_key_symptoms: missing,
})

const diff = (over: Partial<DifferentiationStructured>): DifferentiationStructured => ({
  primary: null,
  concurrent: [],
  transformations: [],
  ...over,
})

describe('nearHints（未定证时「还差哪些表现就能定证」）', () => {
  it('已定证时返回空：定证了再列「还缺什么」是自相矛盾的', () => {
    const d = diff({
      primary: cand('spleen_stomach_damp_heat', '脾胃湿热', []) as any,
      near: [cand('stomach_fire', '胃火炽盛', ['胃脘灼痛'])],
    })
    expect(nearHints(d)).toEqual([])
  })

  it('未定证时列出每个接近候选缺哪些主症', () => {
    const d = diff({
      near: [
        cand('liver_fire_flaring', '肝火上炎', ['头痛眩晕', '面红目赤', '口苦']),
        cand('wind_heat_attack_lung', '风热犯肺证', ['咽痛', '微恶风']),
      ],
    })
    expect(nearHints(d)).toEqual([
      { slug: 'liver_fire_flaring', name: '肝火上炎', missing: ['头痛眩晕', '面红目赤', '口苦'] },
      { slug: 'wind_heat_attack_lung', name: '风热犯肺证', missing: ['咽痛', '微恶风'] },
    ])
  })

  it('缺主症为空的候选不展示：列出来是「什么都不缺」，没有意义', () => {
    const d = diff({
      near: [
        cand('a', '甲证', []),
        cand('b', '乙证', undefined),
        cand('c', '丙证', ['头痛']),
      ],
    })
    expect(nearHints(d)).toEqual([{ slug: 'c', name: '丙证', missing: ['头痛'] }])
  })

  it('候选再多也只给前三条：一次给人看三条够了', () => {
    const d = diff({
      near: [1, 2, 3, 4, 5].map(i => cand(`s${i}`, `证${i}`, [`缺${i}`])),
    })
    expect(nearHints(d)).toHaveLength(3)
    expect(nearHints(d, 5)).toHaveLength(5)
  })

  it('没有结构化辨证时不炸：响应缺字段是常态，不是异常', () => {
    expect(nearHints(undefined)).toEqual([])
    expect(nearHints(null)).toEqual([])
    expect(nearHints(diff({}))).toEqual([])
  })
})
