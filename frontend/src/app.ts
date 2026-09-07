import { createElement, PropsWithChildren } from 'react'
import { ErrorBoundary } from './components/ErrorBoundary'
import './app.scss'

/**
 * 根组件只做一件事：挂全局错误边界。
 *
 * 不包的话，任一页面在渲染期抛错就是整页白屏——只剩一个导航栏，
 * 用户既不知道发生了什么，也没有任何可点的出口。
 *
 * 这里用 `createElement` 而不是 JSX：本文件是 `app.ts`（不是 `.tsx`），
 * 而它是 Taro 的固定入口文件名，不能为了写 JSX 去改。
 */
function App({ children }: PropsWithChildren<any>) {
  return createElement(ErrorBoundary, null, children)
}

export default App
