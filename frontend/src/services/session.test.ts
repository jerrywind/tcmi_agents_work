import { describe, it, expect, beforeEach } from 'vitest'
import {
  advanceRound, clearSession, getMessages, getPayload, getProfile, getResult,
  getRound, pushMessage, resetMessages, rollbackSession, setResult, snapshotSession,
  startSession,
} from './session'
import type { DiagnosisResult } from './harness'

/**
 * 会话容器的行为回归。
 *
 * 为什么不测就算了：`payload.round` 是否递增，直接决定后端「达到轮次上限
 * 强制放行」的兜底能不能触发。它错了不会报错，只会让用户被卡在无限追问里——
 * 属于**静默失效**，必须锁住。
 */
describe('session 会话容器', () => {
  beforeEach(() => clearSession())

  it('startSession 重置历史、档案与轮次', () => {
    startSession({ gender: '男', age: 34 })
    pushMessage({ role: 'user', content: '上一轮残留' })
    advanceRound()
    advanceRound()

    startSession({ gender: '女' })
    expect(getMessages()).toHaveLength(0)
    expect(getRound()).toBe(1)
    expect(getResult()).toBeNull()
    expect(getPayload().gender).toBe('女')
  })

  it('payload 带上 round，且只有「补充」才推进轮次', () => {
    startSession({ gender: '男', age: 34 })
    // 首次主诉仍是第 1 轮：pushMessage 也用于首页发起问诊
    pushMessage({ role: 'user', content: '口苦口臭，大便粘滞' })
    expect(getPayload().round).toBe(1)

    // 在已有结论之上补充 → 推进
    advanceRound()
    expect(getPayload().round).toBe(2)
    advanceRound()
    expect(getPayload().round).toBe(3)
  })

  it('递增轮次不得丢掉体质档案字段', () => {
    startSession({ gender: '男', age: 34, region: '广州' })
    advanceRound()
    const p = getPayload()
    expect(p.gender).toBe('男')
    expect(p.age).toBe(34)
    expect(p.region).toBe('广州')
    expect(p.round).toBe(2)
  })

  it('startSession 后能取回档案（问诊页要用其中的既往病史拼首轮消息）', () => {
    expect(getProfile()).toBeNull()
    startSession({ gender: '男', history: '高血压 5 年' })
    expect(getProfile()).toEqual({ gender: '男', history: '高血压 5 年' })
  })

  it('resetMessages 只清历史，档案与轮次都留着', () => {
    // 首诊失败要靠它撤回已推送的主诉，否则重试一次就多发一遍
    startSession({ gender: '女' })
    pushMessage({ role: 'user', content: '主诉' })
    advanceRound()
    resetMessages()
    expect(getMessages()).toHaveLength(0)
    expect(getProfile()!.gender).toBe('女')
    expect(getPayload().round).toBe(2)
  })

  it('rollbackSession 连消息带轮次一起撤回（追问失败重试不该重复发送）', () => {
    // 追问链路是「先 push + advanceRound，再发请求」，失败时两个副作用都已落地。
    // 只撤消息不撤轮次的话，每失败一次就白吃一轮追问预算，
    // 后端的「达到上限强制放行」会在用户还没补充到信息时提前触发。
    startSession({ gender: '男' })
    pushMessage({ role: 'user', content: '主诉' })
    advanceRound()
    const snapshot = snapshotSession()

    pushMessage({ role: 'user', content: '补充：怕冷' })
    advanceRound()
    expect(getMessages()).toHaveLength(2)
    expect(getPayload().round).toBe(3)

    rollbackSession(snapshot)
    expect(getMessages()).toEqual([{ role: 'user', content: '主诉' }])
    expect(getPayload().round).toBe(2)
    // 档案不受回滚影响
    expect(getProfile()!.gender).toBe('男')
  })

  it('rollbackSession 对越界快照取整，轮次不会掉到 1 以下', () => {
    startSession({ gender: '男' })
    rollbackSession({ messages: -1, round: -5 })
    expect(getMessages()).toEqual([])
    expect(getPayload().round).toBe(1)
  })

  it('setResult 把助手输出回灌进历史（供下一轮模型看到）', () => {
    startSession({ gender: '男' })
    pushMessage({ role: 'user', content: '主诉' })
    const r = { steps: [], summary: '助手的追问' } as unknown as DiagnosisResult
    setResult(r)

    expect(getMessages()).toEqual([
      { role: 'user', content: '主诉' },
      { role: 'assistant', content: '助手的追问' },
    ])
    expect(getResult()).toBe(r)
  })
})
