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
    const r = await fetch(`${BASE}/health`)
    return r.ok
  } catch {
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
      /* 保持原始文本（如 /health 返回 ok） */
    }
    return { statusCode: res.status, data }
  }
}

const up = await ping()

describe.skipIf(!up)('harness 契约（需本地 harness :8011）', () => {
  installRealTransport()

  it('GET /health 返回 ok', async () => {
    await expect(h.health()).resolves.toBe('ok')
  })

  it('GET /agents 返回 7 个 capability 与中文名', async () => {
    const r = await h.listAgents()
    expect(r.capabilities).toEqual([
      'inspection',
      'listening',
      'inquiry',
      'palpation',
      'differentiation',
      'safety',
      'treatment',
    ])
    expect(r.names).toHaveLength(7)
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
