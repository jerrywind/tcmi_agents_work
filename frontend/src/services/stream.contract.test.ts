// @vitest-environment node
/**
 * 前端 ↔ harness「流式契约测试」：连真实 harness，验证 `/chat/stream` 的
 * **事件序列不变量**。
 *
 * ## 为什么只测前几帧就断开
 *
 * 一次完整问诊要 100–500 秒，不能进自动化测试。但流式的核心价值恰恰在
 * **最前面这几帧**：`hello`（步骤计划）必须在毫秒级到达，之后才是逐步产出。
 * 所以这里收到骨架与首批 `step_start` 后立刻 `cancel()`——
 * 既验证了契约，又不用等 LLM。
 *
 * 与 `harness.contract.test.ts` 同样：`/health` 不可达时整体 skip 并**告警**，
 * 不会静默通过。
 */
import { describe, it, expect } from 'vitest'

/**
 * 候选地址：**依次探测，取第一个通的**。
 *
 * 只写死 8011 会闹乌龙：本机 8011 被别的进程占着、或容器端口映射没起来时，
 * 整套契约会静默 skip——「会跳过的检查等于没有检查」。
 * 故允许 `VITE_API_BASE` 指定，并先探测 canonical 对外端口 43301，再给旧端口兜底；
 * 全都连不上才 skip 且告警。
 */
const CANDIDATES = [
  process.env.VITE_API_BASE,
  'http://127.0.0.1:43301',
  'http://127.0.0.1:8011',
  'http://127.0.0.1:18011',
].filter(Boolean) as string[]

async function ping(base: string): Promise<boolean> {
  try {
    const r = await fetch(`${base}/health`, { signal: AbortSignal.timeout(5000) })
    return r.ok
  } catch {
    return false
  }
}

let BASE = ''
for (const c of CANDIDATES) {
  if (await ping(c)) {
    BASE = c
    break
  }
}
if (!BASE) {
  console.warn(
    `[stream.contract] 连不上任何候选地址（${CANDIDATES.join('、')}）：` +
      `流式契约将跳过。请启动 harness。`,
  )
}

// `stream.ts` 的 `HARNESS_BASE_URL` 是**模块级常量**（读到的是 import 那一刻的
// `VITE_API_BASE`），故必须先定址再动态 import，否则探测出的地址用不上。
if (BASE) process.env.VITE_API_BASE = BASE
const { streamChat } = await import('./stream')

const up = BASE !== ''

describe.skipIf(!up)('/chat/stream 事件序列契约（需本地 harness :43301）', () => {
  it('首帧是 hello 且带完整步骤计划，安全门必在计划里', async () => {
    const events: any[] = []
    let settled: (v: string[]) => void
    const done = new Promise<string[]>(r => { settled = r })

    const handle = streamChat({
      messages: [{ role: 'user', content: '咳嗽三天，痰黄，咽痛' }],
      payload: { gender: '男', age: 34, round: 1 },
      onEvent: e => {
        events.push(e)
        // 拿到骨架 + 首批 step_start 就收手，不等 LLM
        if (events.filter(x => x.event === 'step_start').length >= 1
          && events.some(x => x.event === 'hello')) {
          settled(events.map(x => x.event))
        }
      },
      onError: err => settled(['ERROR:' + err.message]),
      onDone: () => settled(events.map(x => x.event)),
    })

    const names = await Promise.race([
      done,
      new Promise<string[]>(r => setTimeout(() => r(events.map(x => x.event)), 20000)),
    ])
    handle.cancel()

    // ① 首帧必须是 hello：这是「骨架秒出」的全部意义所在
    expect(names[0]).toBe('hello')

    const hello = events.find(e => e.event === 'hello')
    const plan = hello?.data?.plan ?? []
    expect(Array.isArray(plan)).toBe(true)
    expect(plan.length).toBeGreaterThan(0)

    // ② 计划项字段齐全，且 index 连续（前端按下标归位渲染）
    plan.forEach((p: any, i: number) => {
      expect(typeof p.capability).toBe('string')
      expect(typeof p.zh).toBe('string')
      expect(p.index).toBe(i)
      expect(['collection', 'diagnosis', 'safety', 'treatment']).toContain(p.phase)
    })

    // ③ 安全门不可被配置移除（合规红线）：计划里必须有它
    expect(plan.some((p: any) => p.capability === 'safety')).toBe(true)

    // ④ 每个 step_start 的 index 都能在计划里找到（增量按下标归位的前提）
    for (const e of events.filter(x => x.event === 'step_start')) {
      expect(plan.some((p: any) => p.index === e.data.index)).toBe(true)
    }
  }, 40000)

  it('取消后不再回调（连接真的断开了）', async () => {
    let count = 0
    const handle = streamChat({
      messages: [{ role: 'user', content: '头痛两天' }],
      payload: { round: 1 },
      onEvent: () => { count += 1 },
    })
    handle.cancel()
    await new Promise(r => setTimeout(r, 1500))
    const after = count
    await new Promise(r => setTimeout(r, 1500))
    // 取消后不应再有新事件到达
    expect(count).toBe(after)
  }, 20000)
})
