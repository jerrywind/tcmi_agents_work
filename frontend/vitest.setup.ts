import { vi } from 'vitest'

// jsdom 环境会把 window 设为 global，而 jsdom 本身不带 fetch，可能覆盖掉
// Node 的全局 fetch。这里从 Node 内置的 undici 兜底恢复真实 fetch / FormData / Blob，
// 确保契约测试能真正连到已启动的 harness。
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

const toResult = async (res: any) => {
  const text = await res.text()
  let data: any = text
  try {
    data = text ? JSON.parse(text) : null
  } catch {
    /* 非 JSON 保持原始文本（如 /health 返回 ok） */
  }
  return { statusCode: res.status, data, header: {}, errMsg: '' }
}

/**
 * 全局桩：@tarojs/taro（小程序运行时）在 Node 测试环境中不可用。
 *
 * ⚠️ 两个关键点：
 * 1. 必须用 vi.fn() 包装。若返回普通 async 函数，用例里的
 *      vi.mocked(Taro.request).mockResolvedValue(...)
 *    会拿到没有 mock 方法的普通函数，报
 *      TypeError: mockedRequest.mockImplementation is not a function
 * 2. 必须用 vi.hoisted() 提前创建。vi.mock 的工厂会被提升到文件顶部，
 *    直接引用外部 const 会触发 TDZ（Cannot access before initialization）。
 */
const taro = vi.hoisted(() => ({
  // 网络
  request: vi.fn(),
  uploadFile: vi.fn(),
  // 存储
  getStorageSync: vi.fn(() => ''),
  setStorageSync: vi.fn(),
  removeStorageSync: vi.fn(),
  // 交互反馈
  showToast: vi.fn(),
  showLoading: vi.fn(),
  hideLoading: vi.fn(),
  showModal: vi.fn(),
  // 路由
  navigateTo: vi.fn(),
  redirectTo: vi.fn(),
  reLaunch: vi.fn(),
  navigateBack: vi.fn(),
  switchTab: vi.fn(),
  // 设备能力
  chooseImage: vi.fn(),
}))

// 默认实现：走真实 fetch（契约测试 / e2e 用）。
// 纯单测可用 vi.mocked(Taro.request).mockResolvedValue(...) 覆盖。
taro.request.mockImplementation(async (opts: any) => {
  const url = opts.url
  try {
    const r = await (globalThis as any).fetch(url, {
      method: opts.method,
      headers: { 'Content-Type': 'application/json', ...(opts.header || {}) },
      body: opts.data !== undefined ? JSON.stringify(opts.data) : undefined,
    })
    return toResult(r)
  } catch (e: any) {
    // eslint-disable-next-line no-console
    console.error('[taro-mock] fetch failed:', url, e && (e.stack || e.message))
    throw e
  }
})

taro.uploadFile.mockImplementation(async (opts: any) => {
  const buf = require('node:fs').readFileSync(opts.filePath)
  const fd = new (globalThis as any).FormData()
  fd.append(opts.name, new Blob([buf]), opts.filePath.split('/').pop())
  for (const [k, v] of Object.entries(opts.formData || {})) fd.append(k, String(v))
  const r = await (globalThis as any).fetch(opts.url, { method: 'POST', body: fd })
  return toResult(r)
})

vi.mock('@tarojs/taro', () => ({
  default: {
    request: taro.request,
    uploadFile: taro.uploadFile,
    getStorageSync: taro.getStorageSync,
    setStorageSync: taro.setStorageSync,
    removeStorageSync: taro.removeStorageSync,
    showToast: taro.showToast,
    showLoading: taro.showLoading,
    hideLoading: taro.hideLoading,
    showModal: taro.showModal,
    navigateTo: taro.navigateTo,
    redirectTo: taro.redirectTo,
    reLaunch: taro.reLaunch,
    navigateBack: taro.navigateBack,
    switchTab: taro.switchTab,
    chooseImage: taro.chooseImage,
  },
  // 同时提供具名导出，兼容 `import { request } from '@tarojs/taro'` 的写法
  request: taro.request,
  uploadFile: taro.uploadFile,
  getStorageSync: taro.getStorageSync,
  setStorageSync: taro.setStorageSync,
  removeStorageSync: taro.removeStorageSync,
  showToast: taro.showToast,
  showLoading: taro.showLoading,
  hideLoading: taro.hideLoading,
  showModal: taro.showModal,
  navigateTo: taro.navigateTo,
  redirectTo: taro.redirectTo,
  reLaunch: taro.reLaunch,
  navigateBack: taro.navigateBack,
  switchTab: taro.switchTab,
  chooseImage: taro.chooseImage,
  useRouter: () => ({ params: {} }),
}))
