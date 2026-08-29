import { describe, it, expect, vi, beforeEach } from 'vitest'
import Taro from '@tarojs/taro'
import {
  callSkill, chat, getReport, health, listAgents, listMcpTools, listReports,
  listSkills, mcpRpc, reloadResources, runAgent,
} from './harness'

const mockedRequest = vi.mocked(Taro.request)

beforeEach(() => {
  mockedRequest.mockReset()
})

describe('harnessRequest', () => {
  it('returns body on 2xx', async () => {
    mockedRequest.mockResolvedValue({ statusCode: 200, data: { ok: true } } as any)
    await expect(health()).resolves.toEqual({ ok: true })
  })

  it('throws detail on >=400', async () => {
    mockedRequest.mockResolvedValue({ statusCode: 400, data: { detail: '参数错误' } } as any)
    await expect(listAgents()).rejects.toThrow('参数错误')
  })

  it('falls back to HTTP status when no detail', async () => {
    mockedRequest.mockResolvedValue({ statusCode: 500, data: {} } as any)
    await expect(listSkills()).rejects.toThrow('HTTP 500')
  })

  /**
   * harness 的错误统一以 200 + {"error": "..."} 返回，
   * 客户端必须识别，否则调用方会把错误当成正常结果。
   */
  it('treats 200 + {error} as an error', async () => {
    mockedRequest.mockResolvedValue({
      statusCode: 200,
      data: { error: 'LLM 不可用' },
    } as any)
    await expect(chat([{ role: 'user', content: 'hi' }])).rejects.toThrow('LLM 不可用')
  })

  it('throws network error when the transport rejects', async () => {
    mockedRequest.mockRejectedValue(new Error('boom'))
    await expect(health()).rejects.toThrow('网络异常')
  })
})

describe('endpoint wiring', () => {
  it('POST /chat sends messages and payload', async () => {
    mockedRequest.mockResolvedValue({
      statusCode: 200,
      data: { steps: [], summary: 'ok' },
    } as any)
    const msgs = [{ role: 'user' as const, content: '口苦口臭' }]
    await chat(msgs, { age: 34 })
    const arg: any = mockedRequest.mock.calls[0][0]
    expect(arg.method).toBe('POST')
    expect(String(arg.url).endsWith('/chat')).toBe(true)
    expect(arg.data).toEqual({ messages: msgs, payload: { age: 34 } })
  })

  it('POST /agents sends capability', async () => {
    mockedRequest.mockResolvedValue({
      statusCode: 200,
      data: { capability: 'differentiation', content: 'x' },
    } as any)
    await runAgent('differentiation', [{ role: 'user', content: 'a' }])
    const arg: any = mockedRequest.mock.calls[0][0]
    expect(String(arg.url).endsWith('/agents')).toBe(true)
    expect(arg.data.capability).toBe('differentiation')
  })

  it('POST /skills sends name and arguments', async () => {
    mockedRequest.mockResolvedValue({ statusCode: 200, data: { result: 1 } } as any)
    await callSkill('tcm-kb', { query: '脾胃湿热' })
    const arg: any = mockedRequest.mock.calls[0][0]
    expect(String(arg.url).endsWith('/skills')).toBe(true)
    expect(arg.data).toEqual({ name: 'tcm-kb', arguments: { query: '脾胃湿热' } })
  })

  it('POST /reload targets /reload', async () => {
    mockedRequest.mockResolvedValue({ statusCode: 200, data: { ok: true } } as any)
    await reloadResources()
    const arg: any = mockedRequest.mock.calls[0][0]
    expect(String(arg.url).endsWith('/reload')).toBe(true)
  })

  it('POST /mcp wraps a JSON-RPC request', async () => {
    mockedRequest.mockResolvedValue({
      statusCode: 200,
      data: { jsonrpc: '2.0', id: 1, result: { tools: [] } },
    } as any)
    await listMcpTools()
    const arg: any = mockedRequest.mock.calls[0][0]
    expect(String(arg.url).endsWith('/mcp')).toBe(true)
    expect(arg.data).toEqual({
      jsonrpc: '2.0', id: 1, method: 'tools/list', params: {},
    })
  })

  // T5.1 报告持久化
  it('GET /reports sends optional limit', async () => {
    mockedRequest.mockResolvedValue({
      statusCode: 200,
      data: { reports: [], enabled: true },
    } as any)
    await listReports(5)
    const arg: any = mockedRequest.mock.calls[0][0]
    expect(String(arg.url).endsWith('/reports?limit=5')).toBe(true)
    expect(arg.method).toBe('GET')
  })

  it('GET /reports omits limit when not given', async () => {
    mockedRequest.mockResolvedValue({
      statusCode: 200,
      data: { reports: [], enabled: false, hint: '报告持久化未启用' },
    } as any)
    const r = await listReports()
    const arg: any = mockedRequest.mock.calls[0][0]
    expect(String(arg.url).endsWith('/reports')).toBe(true)
    // 未启用时不应抛错（只是空列表），调用方据此隐藏入口
    expect(r.enabled).toBe(false)
    expect(r.hint).toBe('报告持久化未启用')
  })

  it('GET /reports/:id escapes the id', async () => {
    mockedRequest.mockResolvedValue({ statusCode: 200, data: { id: 'a b' } } as any)
    await getReport('a b')
    const arg: any = mockedRequest.mock.calls[0][0]
    expect(String(arg.url).endsWith('/reports/a%20b')).toBe(true)
  })

  it('mcpRpc forwards method, params and id', async () => {
    mockedRequest.mockResolvedValue({
      statusCode: 200,
      data: { jsonrpc: '2.0', id: 'x', result: {} },
    } as any)
    await mcpRpc('tools/call', { name: 'run_agent' }, 'x')
    const arg: any = mockedRequest.mock.calls[0][0]
    expect(arg.data.method).toBe('tools/call')
    expect(arg.data.params).toEqual({ name: 'run_agent' })
    expect(arg.data.id).toBe('x')
  })
})
