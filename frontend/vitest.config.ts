import { defineConfig } from 'vitest/config'

// 契约测试（harness.contract.test.ts）需要连接已启动的 harness。
// 1) 通过 define 在编译期把 VITE_API_BASE 注入 services/harness.ts 的 HARNESS_BASE_URL，
//    指向本地 harness 默认端口 8011（用 127.0.0.1 规避 Windows 上
//    localhost 解析到 IPv6 导致超时的问题）。
// 2) 同时设置 TCM_API_BASE 供 e2e 用例与 vitest.setup.ts 使用。
//
// 注意：22000-22200 是早期 backend 的端口区间，已废弃，勿再回退。
const BACKEND = process.env.TCM_API_BASE || 'http://127.0.0.1:8011'
process.env.TCM_API_BASE = BACKEND

export default defineConfig({
  define: {
    'process.env.VITE_API_BASE': JSON.stringify(BACKEND),
  },
  test: {
    environment: 'jsdom',
    globals: true,
    include: ['src/**/*.{test,spec}.{ts,tsx}'],
    setupFiles: ['./vitest.setup.ts'],
    coverage: {
      provider: 'v8',
      reporter: ['text', 'html', 'lcov'],
      include: ['src/**/*.{ts,tsx}'],
      // 页面组件依赖 Taro 运行时，由端到端/真机验证覆盖，不计入单元测试覆盖率基数；
      // 单元测试聚焦可独立验证的纯逻辑（utils + services）。
      exclude: [
        'src/**/*.{test,spec}.{ts,tsx}',
        'src/types.ts',
        'src/app.ts',
        'src/app.config.ts',
        'src/index.html',
        'src/pages/**',
      ],
    },
  },
})
