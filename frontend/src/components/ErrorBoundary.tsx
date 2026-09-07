import { Component, type ErrorInfo, type ReactNode } from 'react'
import Taro from '@tarojs/taro'
import { View, Text } from '@tarojs/components'

interface Props {
  children: ReactNode
  /** 出错时显示的标题，默认「页面出错了」 */
  title?: string
}

interface State {
  error: Error | null
}

/**
 * 错误边界：任一子树**渲染期**抛错时降级成一块可操作的提示，而不是白屏。
 *
 * 白屏在医疗场景里格外糟糕：用户分不清是自己操作错了、网络断了，
 * 还是结论有问题——屏幕上只剩一个导航栏，什么线索都没有。
 * 至少要让人看见出了什么事、下一步该怎么办。
 *
 * 注意 React 错误边界的能力边界：**只捕获渲染期**的异常。
 * 事件回调、`setTimeout`、Promise 里的错误它兜不住，那些仍要调用方 try/catch。
 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null }

  static getDerivedStateFromError(error: Error): State {
    return { error }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // 完整堆栈留给控制台，页面上只给人看结论
    console.error('[ErrorBoundary]', error, info.componentStack)
  }

  /** 回首页重新开始：会话状态本来就在内存里，出错后接着用多半也是错的 */
  private restart = () => {
    Taro.reLaunch({ url: '/pages/index/index' })
  }

  render() {
    if (!this.state.error) return this.props.children
    return (
      <View className='error-fallback'>
        <Text className='error-title'>{this.props.title || '页面出错了'}</Text>
        <Text className='error-detail'>{this.state.error.message || '未知错误'}</Text>
        <View className='btn-primary' onClick={this.restart}>
          回到首页重新问诊
        </View>
      </View>
    )
  }
}
