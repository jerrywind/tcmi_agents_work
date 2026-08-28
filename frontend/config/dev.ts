import type { UserConfigExport } from '@tarojs/cli'

export default {
  logger: { quiet: false, stats: true },
  mini: {},
  h5: {
    devServer: {
      port: 10086,
      proxy: {
        // harness（Rust 后端）监听 8011；其端点无 /api 前缀，故代理时剥离。
        '/api': {
          target: 'http://127.0.0.1:8011',
          changeOrigin: true,
          pathRewrite: { '^/api': '' }
        }
      }
    }
  }
} satisfies UserConfigExport<'webpack5'>
