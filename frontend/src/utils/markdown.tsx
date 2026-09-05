import { Fragment, type ReactNode } from 'react'
import { View, Text } from '@tarojs/components'

/**
 * 轻量 Markdown 渲染器（Taro 多端）。
 *
 * 后端 `/chat` 的各步 `text` 与 `summary` 都是 Markdown（见 `orchestrator.rs`
 * 里 `final_text` 用 `## 望诊` 这种标题拼装），此前前端用 `<Text>` 整段平铺，
 * 标题、列表、加粗全都没有层次，读起来像一坨纯文本。
 *
 * 不引第三方库：react-markdown 依赖 DOM，小程序端跑不起来；`<RichText>` 的
 * HTML 节点在小程序里标签支持残缺。这里直接把常用子集（标题 / 有序无序列表 /
 * 引用 / 加粗 / 斜体 / 行内代码 / 段落 / 换行）解析成 Taro 组件，三端通用。
 *
 * 支持范围刻意只覆盖 LLM 实际会产出的语法，不追求完整 CommonMark。
 */

/** 行内解析：**加粗** / *斜体* / `行内代码`。 */
function renderInline(text: string): ReactNode[] {
  const nodes: ReactNode[] = []
  // 顺序很重要：先匹配 ** 再匹配 *，避免把 ** 拆成两个斜体
  const re = /(\*\*([^*]+)\*\*|\*([^*]+)\*|`([^`]+)`)/g
  let last = 0
  let m: RegExpExecArray | null
  let key = 0
  while ((m = re.exec(text)) !== null) {
    if (m.index > last) {
      nodes.push(<Fragment key={key++}>{text.slice(last, m.index)}</Fragment>)
    }
    if (m[2] !== undefined) {
      nodes.push(<Text key={key++} className='md-bold'>{m[2]}</Text>)
    } else if (m[3] !== undefined) {
      nodes.push(<Text key={key++} className='md-italic'>{m[3]}</Text>)
    } else if (m[4] !== undefined) {
      nodes.push(<Text key={key++} className='md-code'>{m[4]}</Text>)
    }
    last = re.lastIndex
  }
  if (last < text.length) {
    nodes.push(<Fragment key={key++}>{text.slice(last)}</Fragment>)
  }
  return nodes
}

const BLOCK_RE = /^(#{1,4})\s+(.*)$|^(>\s?)(.*)$|^[-*]\s+(.*)$|^(\d+)\.\s+(.*)$/

interface ListBuffer {
  ordered: boolean
  items: string[]
}

export function Markdown({ text, className }: { text: string; className?: string }) {
  if (!text) return null
  const lines = text.split('\n')
  const blocks: ReactNode[] = []
  let key = 0
  let list: ListBuffer | null = null

  const flushList = () => {
    if (!list) return
    const items = list.items.map((it, idx) => (
      <View key={idx} className='md-li'>
        <Text className='md-li-mark'>{list!.ordered ? `${idx + 1}.` : '•'}</Text>
        <Text className='md-li-text'>{renderInline(it)}</Text>
      </View>
    ))
    blocks.push(
      <View key={key++} className={list.ordered ? 'md-ol' : 'md-ul'}>
        {items}
      </View>,
    )
    list = null
  }

  let i = 0
  while (i < lines.length) {
    const line = lines[i]
    const h = /^(#{1,4})\s+(.*)$/.exec(line)
    if (h) {
      flushList()
      blocks.push(
        <View key={key++} className={`md-h md-h${h[1].length}`}>
          {renderInline(h[2])}
        </View>,
      )
      i++
      continue
    }
    const bq = /^>\s?(.*)$/.exec(line)
    if (bq) {
      flushList()
      const inner: ReactNode[] = []
      while (i < lines.length) {
        const b = /^>\s?(.*)$/.exec(lines[i])
        if (!b) break
        inner.push(<Text key={inner.length}>{renderInline(b[1])}</Text>)
        i++
      }
      blocks.push(<View key={key++} className='md-quote'>{inner}</View>)
      continue
    }
    const ul = /^[-*]\s+(.*)$/.exec(line)
    if (ul) {
      if (!list || list.ordered) {
        flushList()
        list = { ordered: false, items: [] }
      }
      list.items.push(ul[1])
      i++
      continue
    }
    const ol = /^(\d+)\.\s+(.*)$/.exec(line)
    if (ol) {
      if (!list || !list.ordered) {
        flushList()
        list = { ordered: true, items: [] }
      }
      list.items.push(ol[2])
      i++
      continue
    }
    if (line.trim() === '') {
      flushList()
      i++
      continue
    }
    // 段落：聚合到下一个空行或块级语法
    const para: string[] = []
    while (
      i < lines.length &&
      lines[i].trim() !== '' &&
      !BLOCK_RE.test(lines[i])
    ) {
      para.push(lines[i])
      i++
    }
    flushList()
    blocks.push(<View key={key++} className='md-p'>{renderInline(para.join('\n'))}</View>)
  }
  flushList()

  return <View className={className}>{blocks}</View>
}
