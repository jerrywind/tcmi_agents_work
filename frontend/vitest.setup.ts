import { vi } from 'vitest'

// 全局桩：@tarojs/taro（小程序运行时）在 Node 测试环境中不可用，
// 用 vi.fn 替身确保 services/api.ts 可独立单元测试。
vi.mock('@tarojs/taro', () => ({
  default: {
    request: vi.fn(),
    uploadFile: vi.fn(),
    getStorageSync: vi.fn(() => ''),
    setStorageSync: vi.fn(),
  },
  request: vi.fn(),
  uploadFile: vi.fn(),
}))
