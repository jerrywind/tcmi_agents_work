import type { UserConfigExport } from '@tarojs/cli'

export default {
  logger: { quiet: false, stats: true },
  mini: {},
  h5: {
    devServer: {
      port: 10086,
      // SSE 必须关掉压缩：dev-server 的 gzip 中间件会缓冲响应，
      // `/api/chat/stream` 的事件会被攒着一起发，流式在开发环境里白做。
      // （生产环境同理，见 deploy/nginx/frontend.conf 的 `gzip off`。）
      compress: false,
      proxy: {
        // harness（Rust 后端 tcmi_server）对外监听 43301；其端点无 /api 前缀，故代理时剥离。
        // `HARNESS_DEV_PORT` 可临时改指向：本机 43301 被占（或容器端口映射没起来）
        // 时不必改代码，起服务时带一下即可。
        '/api': {
          target: process.env.HARNESS_DEV_PORT
            ? `http://127.0.0.1:${process.env.HARNESS_DEV_PORT}`
            : 'http://127.0.0.1:43301',
          changeOrigin: true,
          pathRewrite: { '^/api': '' }
        }
      }
    }
  }
} satisfies UserConfigExport<'webpack5'>
