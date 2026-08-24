import { defineConfig } from 'vitest/config'

export default defineConfig({
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
