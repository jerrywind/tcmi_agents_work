import { describe, expect, it } from 'vitest'
import { applyStreamEvent, emptyAcc } from './streamAcc'

/**
 * 累加器的确定性回归。
 *
 * 这段逻辑决定「用户看到什么」，原先埋在 consult 页里无从断言；
 * 抽成纯函数后才能守住下面几条**真机才暴露过**的行为。
 */
describe('applyStreamEvent', () => {
  it('逐步拼接 token 增量', () => {
    let acc = emptyAcc()
    acc = applyStreamEvent(acc, { event: 'step_delta', data: { index: 2, delta: '舌红' } })
    acc = applyStreamEvent(acc, { event: 'step_delta', data: { index: 2, delta: '苔黄腻' } })
    expect(acc.partial[2]).toBe('舌红苔黄腻')
    expect(acc.states[2]).toBe('running')
  })

  it('不同步骤的下标互不串台', () => {
    let acc = emptyAcc()
    acc = applyStreamEvent(acc, { event: 'step_delta', data: { index: 0, delta: '甲' } })
    acc = applyStreamEvent(acc, { event: 'step_delta', data: { index: 3, delta: '乙' } })
    expect(acc.partial[0]).toBe('甲')
    expect(acc.partial[3]).toBe('乙')
  })

  /**
   * L5 的核心：重试会把正文**从头再推一遍**，不清空就拼出两份。
   * 真机实测一次开方步 361s 全耗在重试上，这条守的就是它。
   */
  it('重试时丢弃已累积的半截正文并记录重试进度', () => {
    let acc = emptyAcc()
    acc = applyStreamEvent(acc, { event: 'step_delta', data: { index: 1, delta: '前半段' } })
    acc = applyStreamEvent(acc, {
      event: 'step_retry',
      data: { index: 1, attempt: 1, max_retries: 2, error: '读取 LLM 流式响应失败' },
    })
    expect(acc.partial[1]).toBeUndefined()
    expect(acc.states[1]).toBe('running')
    expect(acc.retries[1]).toEqual({
      attempt: 1, maxRetries: 2, error: '读取 LLM 流式响应失败',
    })

    // 重试后推来的新正文从零开始累积，不能与旧的拼在一起
    acc = applyStreamEvent(acc, { event: 'step_delta', data: { index: 1, delta: '新正文' } })
    expect(acc.partial[1]).toBe('新正文')
  })

  it('步骤完成/失败后清除重试标记', () => {
    let acc = emptyAcc()
    acc = applyStreamEvent(acc, {
      event: 'step_retry', data: { index: 4, attempt: 2, max_retries: 2, error: 'x' },
    })
    acc = applyStreamEvent(acc, {
      event: 'step_done',
      data: { index: 4, capability: 'prescription', zh: '开方', text: '连朴饮' },
    })
    expect(acc.retries[4]).toBeUndefined()
    expect(acc.partial[4]).toBeUndefined()
    expect(acc.texts[4].text).toBe('连朴饮')
    expect(acc.gotStep).toBe(true)

    // 失败同样要清掉，否则界面会一直挂着「重试 2/2」
    acc = applyStreamEvent(acc, {
      event: 'step_retry', data: { index: 5, attempt: 1, max_retries: 2, error: 'x' },
    })
    acc = applyStreamEvent(acc, {
      event: 'step_fail', data: { index: 5, capability: 'herbology', error: '超时' },
    })
    expect(acc.retries[5]).toBeUndefined()
    expect(acc.states[5]).toBe('failed')
    expect(acc.failures[0]).toEqual({ capability: 'herbology', error: '超时' })
  })

  it('已完成的状态不被迟到的 step_start 覆盖', () => {
    let acc = emptyAcc()
    acc = applyStreamEvent(acc, {
      event: 'step_done', data: { index: 0, capability: 'inspection', zh: '望诊', text: 't' },
    })
    acc = applyStreamEvent(acc, { event: 'step_start', data: { index: 0 } })
    expect(acc.states[0]).toBe('done')
  })

  it('拦截与跳过按约定落位', () => {
    let acc = emptyAcc()
    acc = applyStreamEvent(acc, {
      event: 'blocked',
      data: { slug: 'chest_pain', label: '胸痛', severity: 'high', advice: '立即就医' },
    })
    expect(acc.blocked).toEqual({
      slug: 'chest_pain', label: '胸痛', severity: 'high', advice: '立即就医',
    })
    acc = applyStreamEvent(acc, {
      event: 'skipped',
      data: { capabilities: [{ capability: 'prescription' }], reason: '已拦截' },
    })
    expect(acc.skipped).toEqual([{ capability: 'prescription', reason: '已拦截' }])
  })

  it('未知事件被忽略（后端加事件不至于搞崩老前端）', () => {
    const before = emptyAcc()
    const after = applyStreamEvent(before, { event: 'something_new', data: { a: 1 } })
    expect(after).toBe(before)
  })
})
