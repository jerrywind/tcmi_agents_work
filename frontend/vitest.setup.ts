import { vi } from 'vitest'

// jsdom 环境会把 window 设为 global，而 jsdom 本身不带 fetch，可能覆盖掉
// Node 的全局 fetch。这里从 Node 内置的 undici 兜底恢复真实 fetch / FormData / Blob，
// 确保全链路 e2e 能真正连到已启动的 backend。
try {
  // Node 18+ 内置 undici 提供 fetch/FormData/Blob/Headers 等
  // @ts-ignore
  const undici = require('undici')
  if (typeof (globalThis as any).fetch === 'undefined' && undici.fetch) {
    ;(globalThis as any).fetch = undici.fetch
  }
  if (typeof (globalThis as any).FormData === 'undefined' && undici.FormData) {
    ;(globalThis as any).FormData = undici.FormData
  }
  if (typeof (globalThis as any).Blob === 'undefined' && undici.Blob) {
    ;(globalThis as any).Blob = undici.Blob
  }
} catch {
  // undici 不可用时退回 Node 全局 fetch
}


// 全局桩：@tarojs/taro（小程序运行时）在 Node 测试环境中不可用，
// 用真实 fetch 适配器替身，使 services/api.ts 既能单元自测，也能在
// e2e 模式下真正连到 VITE_API_BASE 指向的后端。
// 注意：api.ts 已通过 vitest.config 的 define 把 BASE_URL 注入为完整 host，
// 此处 mock 直接对 opts.url（已是完整 URL）发起真实 fetch 即可。
const toResult = async (res: any) => {
  const text = await res.text()
  let data: any = text
  try {
    data = JSON.parse(text)
  } catch {
    /* 非 JSON 保持文本 */
  }
  return { statusCode: res.status, data, header: {}, errMsg: '' }
}

vi.mock('@tarojs/taro', () => ({
  default: {
    request: async (opts: any) => {
      // api.ts 已通过 VITE_API_BASE 注入完整 host，opts.url 即为完整 URL
      const url = opts.url
      try {
        const r = await (globalThis as any).fetch(url, {
          method: opts.method,
          headers: { 'Content-Type': 'application/json' },
          body: opts.data ? JSON.stringify(opts.data) : undefined,
        })
        return toResult(r)
      } catch (e: any) {
        // eslint-disable-next-line no-console
        console.error('[taro-mock] fetch failed:', url, e && (e.stack || e.message))
        throw e
      }
    },
    uploadFile: async (opts: any) => {
      const buf = require('node:fs').readFileSync(opts.filePath)
      const fd = new (globalThis as any).FormData()
      fd.append(opts.name, new Blob([buf]), opts.filePath.split('/').pop())
      for (const [k, v] of Object.entries(opts.formData || {})) fd.append(k, String(v))
      const r = await (globalThis as any).fetch(opts.url, { method: 'POST', body: fd })
      return toResult(r)
    },
    getStorageSync: vi.fn(() => ''),
    setStorageSync: vi.fn(),
  },
  request: async (opts: any) => {
    const url = opts.url
    const r = await (globalThis as any).fetch(url, {
      method: opts.method,
      headers: { 'Content-Type': 'application/json' },
      body: opts.data ? JSON.stringify(opts.data) : undefined,
    })
    return toResult(r)
  },
  uploadFile: async (opts: any) => {
    const buf = require('node:fs').readFileSync(opts.filePath)
    const fd = new (globalThis as any).FormData()
    fd.append(opts.name, new Blob([buf]), opts.filePath.split('/').pop())
    for (const [k, v] of Object.entries(opts.formData || {})) fd.append(k, String(v))
    const r = await (globalThis as any).fetch(opts.url, { method: 'POST', body: fd })
    return toResult(r)
  },
}))
