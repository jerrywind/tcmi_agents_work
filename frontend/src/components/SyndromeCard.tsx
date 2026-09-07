import { View, Text } from '@tarojs/components'
import { confidencePercent } from '../utils/format'
import type { SyndromeAssessment } from '../types'

/**
 * 单个证候卡片：证名 + 置信度 + 支持/矛盾证据（T4.1）。
 *
 * 兼证与主证用同一组件渲染，只是标签不同（T4.2）——并存关系要在视觉上等权。
 *
 * 抽成共享组件是因为**两个页面都要用**：报告页的完整展示，以及问诊页
 * 流式进行中的就地插入。此前只在报告页有一份，若问诊页再抄一份，
 * 两处展示口径迟早漂移（「同一个证候在两页长得不一样」）。
 */
export function SyndromeCard({
  kind, s, compact,
}: {
  kind: string
  s: SyndromeAssessment
  /** 流式进行中用紧凑版：少占屏幕，正文还在下面长 */
  compact?: boolean
}) {
  return (
    <View className={`chain-block ${compact ? 'chain-block-compact' : ''}`}>
      <View className='syndrome-row'>
        <View>
          <Text className='chain-name'>{s.name}</Text>
          <Text className='sub-title'>{kind}</Text>
        </View>
        <Text className='syndrome-conf'>{confidencePercent(s.confidence)}</Text>
      </View>
      <View className='chain-group'>
        <Text className='chain-label sup'>支持</Text>
        {s.supporting.length
          ? s.supporting.map((e, i) => <Text key={`sup_${i}`} className='ev-tag'>{e}</Text>)
          : <Text className='rv-empty'>（无）</Text>}
      </View>
      <View className='chain-group'>
        <Text className='chain-label con'>矛盾</Text>
        {s.conflicting.length
          ? s.conflicting.map((e, i) => <Text key={`con_${i}`} className='ev-tag con'>{e}</Text>)
          : <Text className='rv-empty'>（无）</Text>}
      </View>
      {!compact && s.pathogenesis
        ? <Text className='note-text'>病机：{s.pathogenesis}</Text>
        : null}
    </View>
  )
}
