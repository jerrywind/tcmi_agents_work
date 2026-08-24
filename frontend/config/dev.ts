import type { UserConfigExport } from '@tarojs/cli'

export default {
  logger: { quiet: false, stats: true },
  mini: {},
  h5: {
    devServer: {
      port: 10086,
      proxy: {
        '/api': { target: 'http://127.0.0.1:8000', changeOrigin: true },
        '/uploads': { target: 'http://127.0.0.1:8000', changeOrigin: true }
      }
    }
  }
} satisfies UserConfigExport<'webpack5'>
