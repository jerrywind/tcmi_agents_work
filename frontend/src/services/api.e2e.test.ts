/**
 * 前端 service 层 ↔ 后端 全链路 e2e（函数级，无需浏览器）。
 *
 * 真实执行 src/services/api.ts 中的导出函数（createConsultation / startConsultation
 * / answerQuestion / getState / getReport / getStream ...），把 @tarojs/taro 的
 * request/uploadFile 适配层替换为「真实 fetch 到已启动的 backend」，从而验证：
 *   - 前端 api.ts 的请求路径、body 结构与后端契约一致；
 *   - 从前端视角发起的一次完整问诊能驱动到 finished 并拿到报告/证据/轨迹。
 *
 * 运行前需先启动 backend（建议开启 mock 模式，无需真实 LLM）：
 *   cd backend && python -m uvicorn app.main:app --port 8000
 * 然后通过环境变量 TCM_API_BASE 指向它：
 *   TCM_API_BASE=http://localhost:8000 npx vitest run src/services/api.e2e.test.ts
 *
 * 该测试会在 backend 上写入一个真实会话，属于集成测试，默认在 vitest 的
 * 非 watch 模式下运行。
 */
import { describe, it, expect } from 'vitest'

// ---- 后端地址由 vitest.config.ts 通过 define 注入 VITE_API_BASE（22000 端口）----
// 注：@tarojs/taro 的 request/uploadFile 真实 fetch 适配器在 vitest.setup.ts
// 中统一注入，此处无需重复 mock。

import * as api from './api'

// 无需调用任何 mock 设置接口：backend 在 TCM_LLM_BASE_URL 留空时自动回退
// MockProvider（规则兜底），整条问诊可离线收敛到 finished。

describe('前端 service 层 ↔ 后端 全链路', () => {
  let cid: string

  it('创建会话并驱动到 finished', async () => {
    const state: any = await api.createConsultation(
      { gender: '男' } as any, 'e2e-前端链路-失眠乏力', {}, '', '',
    )
    expect(state.id).toBeTruthy()
    cid = state.id

    await api.startConsultation(cid)

    let cur: any = await api.getState(cid)
    let guard = 0
    // 多轮问诊：waiting_answer（望闻问切）与 treatment_qa（诊疗方案个性化）都需要逐轮作答
    while (['waiting_answer', 'treatment_qa'].includes(cur.status) && guard < 20) {
      const q = cur.question
      if (!q || !q.id) break
      const opts: any[] = q?.options || [{ value: '无' }]
      cur = await api.answerQuestion(cid, q.id, opts[0].value)
      guard++
    }
    expect(['finished', 'referred']).toContain(cur.status)
  }, 60000)

  it('获取报告 / 流式轨迹 / 护理清单', async () => {
    expect(cid).toBeTruthy()
    const finalState: any = await api.getState(cid)
    const report: any = finalState.report
    expect(report && Object.keys(report).length).toBeTruthy()

    const stream: any = await api.getStream(cid, 0)
    expect(['running', 'done', 'error']).toContain(stream.task)

    const care: any[] = await api.getCare(cid)
    expect(Array.isArray(care)).toBe(true)
  }, 30000)
})
