import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import Taro from '@tarojs/taro'
import {
  advanceRound, clearSession, getMessages, getPayload, getProfile, getResult,
  getRound, pushMessage, resetMessages, rollbackSession, setResult, snapshotSession,
  SESSION_KEY, startSession,
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

/**
 * 本地快照的回归。
 *
 * 一次标准档 `/chat` 要跑 200–530 秒。刷新/切后台把会话全丢 = 白等一整轮，
 * 这是用户侧最贵的一次等待。快照规则：
 * - 每次状态变更落 Storage；
 * - 模块加载时，**有完整 result** → 恢复历史与轮次；**没有 result**（进行中被打断）
 *   → 只留档案，清空消息（否则用户重新填主诉会 append 出「两遍主诉」）。
 *
 * mock 用的是 `vitest.setup.ts` 里 hoisted 的同一批 vi.fn，resetModules 后仍指向
 * 同一对象，所以 beforeEach 里装的 store 对重新 import 的模块同样生效。
 */
describe('session 本地快照（刷新不白等）', () => {
  let store: Record<string, string> = {}

  /** 重新加载模块：等价于「刷新页面后首次 import」时的 hydrate */
  async function freshSession() {
    vi.resetModules()
    const m = await import('./session')
    return m
  }

  beforeEach(() => {
    store = {}
    vi.mocked(Taro.setStorageSync).mockImplementation((k, v) => { store[String(k)] = String(v) })
    vi.mocked(Taro.getStorageSync).mockImplementation(k => store[String(k)] ?? '')
    vi.mocked(Taro.removeStorageSync).mockImplementation(k => { delete store[String(k)] })
  })

  afterEach(() => {
    vi.mocked(Taro.setStorageSync).mockImplementation(() => undefined)
    vi.mocked(Taro.getStorageSync).mockImplementation(() => '')
    vi.mocked(Taro.removeStorageSync).mockImplementation(() => undefined)
  })

  it('每次状态变更都把快照落进 Storage（不含图片等大字段，纯 JSON）', () => {
    clearSession()
    startSession({ gender: '男', history: '高血压' })
    pushMessage({ role: 'user', content: '咳嗽痰黄' })
    advanceRound()
    const r = { steps: [{ capability: 'differentiation', text: '## 辨证' }], summary: '摘要' } as unknown as DiagnosisResult
    setResult(r)

    const saved = JSON.parse(store[SESSION_KEY])
    expect(saved.profile).toEqual({ gender: '男', history: '高血压' })
    expect(saved.round).toBe(2)
    expect(saved.messages).toEqual([
      { role: 'user', content: '咳嗽痰黄' },
      { role: 'assistant', content: '摘要' },
    ])
    expect(saved.result.summary).toBe('摘要')
  })

  it('有完整 result 的快照在重新加载时恢复历史、轮次与结论（刷新后可直接继续追问）', async () => {
    // 预置一个「上一次完整跑到结论」的快照
    store[SESSION_KEY] = JSON.stringify({
      profile: { gender: '男', age: 40 },
      payload: { gender: '男', age: 40 },
      round: 2,
      messages: [
        { role: 'user', content: '咳嗽痰黄' },
        { role: 'assistant', content: '摘要 A' },
        { role: 'user', content: '补充怕冷' },
      ],
      result: { steps: [], summary: '摘要 B' },
    })

    const s = await freshSession()
    expect(s.getProfile()).toEqual({ gender: '男', age: 40 })
    expect(s.getMessages()).toHaveLength(3)
    expect(s.getRound()).toBe(2)
    expect(s.getResult()).toEqual({ steps: [], summary: '摘要 B' })
    expect(s.consumeSessionNotice()).toBe('restored')
    // 提示只能消费一次
    expect(s.consumeSessionNotice()).toBeNull()
  })

  it('进行中被中断（无 result）的快照只留档案，消息清空、轮次归 1', async () => {
    store[SESSION_KEY] = JSON.stringify({
      profile: { gender: '女', region: '广州' },
      payload: { gender: '女', region: '广州' },
      round: 3,
      messages: [
        { role: 'user', content: '半截主诉' },
        { role: 'assistant', content: '跑了一半的步骤' },
      ],
      result: null,
    })

    const s = await freshSession()
    expect(s.getProfile()).toEqual({ gender: '女', region: '广州' })
    // 关键：不清空的话，用户重新填主诉会 append 成「两遍主诉」
    expect(s.getMessages()).toEqual([])
    expect(s.getRound()).toBe(1)
    expect(s.getResult()).toBeNull()
    expect(s.consumeSessionNotice()).toBe('interrupted')
  })

  it('坏数据/空数据不炸，按无快照处理', async () => {
    store[SESSION_KEY] = '{ 不是合法 JSON'

    const s = await freshSession()
    expect(s.getProfile()).toBeNull()
    expect(s.consumeSessionNotice()).toBeNull()
  })

  it('startSession 会覆盖快照（重新建档即开始新问诊，旧结论不再恢复）', async () => {
    // 预置上一次完整结论
    store[SESSION_KEY] = JSON.stringify({
      profile: { gender: '男' },
      payload: { gender: '男' },
      round: 2,
      messages: [{ role: 'user', content: '主诉' }],
      result: { steps: [], summary: '旧结论' },
    })

    const s = await freshSession()
    expect(s.getResult()).toEqual({ steps: [], summary: '旧结论' })

    // 用户从档案页重新建档 → 旧会话被覆盖为全新会话
    s.startSession({ gender: '男' })
    expect(s.getResult()).toBeNull()
    expect(s.getMessages()).toEqual([])
    const saved = JSON.parse(store[SESSION_KEY])
    expect(saved.result).toBeNull()
  })
})
