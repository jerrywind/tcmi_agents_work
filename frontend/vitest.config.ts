import { defineConfig } from 'vitest/config'

// 全链路 e2e 测试需要连接已启动的 backend。
// 1) 通过 define 在编译期把 VITE_API_BASE 注入 api.ts 的 BASE_URL，
//    指向本地 Docker 后端 22000 端口（用 127.0.0.1 规避 Windows 上
//    localhost 解析到 IPv6 导致超时的问题）。
// 2) 同时设置 TCM_API_BASE 供 vitest.setup.ts 中的 Taro mock 使用。
const BACKEND = 'http://127.0.0.1:22000'
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
