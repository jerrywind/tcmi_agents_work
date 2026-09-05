// @vitest-environment node
/**
 * 前端 ↔ harness「契约测试」：用真实 Node fetch 替身替换 Taro 运行时，
 * 让 src/services/harness.ts 的客户端直连真实 harness 服务，验证契约一致。
 *
 * 设计：
 * - node 环境，保证 fetch 为真实 Node 实现。
 * - 顶层探测 /health；不可达则整个 describe 自动 skip，
 *   因此 CI（无后端）下 `npm run test` 仍全绿，本地起 harness 后自动执行。
 * - 只校验**只读端点**（/health、/agents、/skills）：/chat 需要真实 LLM，
 *   不在自动化测试内（harness 无 MockProvider）。
 *
 * 本地运行：
 *   cd server/harness && ../target/debug/harness --listen 127.0.0.1:8011
 *   cd frontend && npx vitest run src/services/harness.contract.test.ts
 */
import { describe, it, expect } from 'vitest'
import Taro from '@tarojs/taro'
import * as h from './harness'

const BASE = process.env.VITE_API_BASE || 'http://127.0.0.1:8011'

async function ping(): Promise<boolean> {
  try {
    // 加超时：连不上时不要卡住整个测试跑——此前没有超时，
    // 端口不通的情形下会一直挂到 vitest 整体超时，报错信息还指向别处。
    const r = await fetch(`${BASE}/health`, { signal: AbortSignal.timeout(5000) })
    return r.ok
  } catch (e) {
    // 会 skip 的测试等于没有测试，至少要让人**看见**它跳过了。
    // 本文件在 CI 里长期「1 skipped」而无人察觉，就是因为这里静默。
    console.warn(
      `[harness.contract] 连不上 ${BASE}（${(e as Error)?.message}）：` +
        `以下真实契约将跳过。请启动 harness，或用 VITE_API_BASE 指向它。`,
    )
    return false
  }
}

function installRealTransport() {
  ;(Taro as any).request = async (opts: any) => {
    const res = await fetch(opts.url, {
      method: opts.method,
      headers: { 'Content-Type': 'application/json', ...(opts.header || {}) },
      body: opts.data !== undefined ? JSON.stringify(opts.data) : undefined,
    })
    const text = await res.text()
    let data: any = text
    try {
      data = text ? JSON.parse(text) : null
    } catch {
      /* 保持原始文本（非 JSON 响应） */
    }
    return { statusCode: res.status, data }
  }
}

const up = await ping()

describe.skipIf(!up)('harness 契约（需本地 harness :8011）', () => {
  installRealTransport()

  it('GET /health 返回 ok 与 RAG 可达性', async () => {
    // T7.5 起 /health 是 JSON：status 只表示进程存活，
    // 典籍检索是否接通单独看 rag 字段。
    const r = await h.health()
    expect(r.status).toBe('ok')
    expect(typeof r.rag?.configured).toBe('boolean')
  })

  it('GET /agents 返回全部 capability 与中文名', async () => {
    const r = await h.listAgents()
    // 望→闻→问→切→医案→辨证→安全门→立法→用药→开方→调护→针灸→治疗
    expect(r.capabilities).toEqual([
      'inspection',
      'listening',
      'inquiry',
      'palpation',
      'case_reference',
      'differentiation',
      'safety',
      'strategy',
      'herbology',
      'prescription',
      'care',
      'acupuncture',
      'treatment',
    ])
    expect(r.names).toHaveLength(r.capabilities.length)
    expect(r.names[0]).toBe('望诊')
  })

  it('GET /skills 返回技能清单（含 name/description/owner）', async () => {
    const r = await h.listSkills()
    expect(Array.isArray(r.skills)).toBe(true)
    expect(r.skills.length).toBeGreaterThan(0)
    for (const s of r.skills) {
      expect(typeof s.name).toBe('string')
      expect(typeof s.owner).toBe('string')
    }
  })

  it('POST /skills 调用未知技能时返回可识别错误', async () => {
    await expect(h.callSkill('__not_exist__', {})).rejects.toThrow()
  })

  // T4.5：MCP Server 对外暴露 7 个能力。只读调用（tools/list、
  // list_agent_capabilities）不需要 LLM，故可纳入契约测试。
  it('POST /mcp tools/list 暴露 7 个 agent_* 工具', async () => {
    const r = await h.listMcpTools()
    expect(r.error).toBeUndefined()
    const tools = r.result?.tools ?? []
    const names: string[] = tools.map((t: any) => t.name)
    for (const cap of [
      'inspection', 'listening', 'inquiry', 'palpation',
      'differentiation', 'safety', 'treatment',
    ]) {
      expect(names).toContain(`agent_${cap}`)
    }
    expect(names).toContain('run_agent')
    expect(names).toContain('list_agent_capabilities')
  })

  it('POST /mcp tools/call 可调用 list_agent_capabilities', async () => {
    const r = await h.mcpRpc('tools/call', { name: 'list_agent_capabilities', arguments: {} }, 2)
    expect(r.id).toBe(2)
    expect(r.result?.isError).toBe(false)
    expect(String(r.result?.content?.[0]?.text)).toContain('differentiation')
  })
})
