import { useEffect, useState } from 'react'
import Taro from '@tarojs/taro'
import { View, Text, ScrollView } from '@tarojs/components'
import { CAPABILITY_ZH } from '../../services/harness'
import { getResult } from '../../services/session'
import { confidencePercent } from '../../utils/format'
import type {
  DiagnosisResult, HarnessCapability, SyndromeAssessment,
} from '../../types'
import './index.scss'

/**
 * 兜底免责声明：服务端未下发 `disclaimer` 时才用本地这份。
 * 正常应由后端统一下发（T5.4），避免各端文案不一致。
 */
const DISCLAIMER_FALLBACK =
  '本内容由 AI 生成，仅供健康参考，不构成医疗诊断或处方建议。如有不适或出现胸痛、咯血、高热不退等警示症状，请及时线下就医。'

/**
 * 存证：把一份报告的文本快照复制到剪贴板（T5.1）。
 *
 * 为什么是复制而不是上传：本系统没有用户体系，前端无从判断「这份报告属于谁」，
 * 把问诊内容发往第三方反而增加隐私风险。复制出来的文本与服务端
 * `GET /reports/:id` 的归档内容一致，用户自行留存即可。
 */
function buildEvidence(result: DiagnosisResult): string {
  const lines = [
    `报告编号：${result.report_id || '（服务端未启用归档）'}`,
    `生成时间：${new Date().toLocaleString()}`,
    `步骤数：${result.steps.length}${result.partial ? '（部分步骤失败，结果不完整）' : ''}`,
    result.blocked ? `安全门拦截：${result.block_reason || ''}` : '',
    '',
    '【辨证结论】',
    result.summary,
  ]
  const structured = result.structured?.differentiation
  if (structured?.primary) {
    lines.push(
      '',
      '【结构化辨证】',
      `主证：${structured.primary.name} ${confidencePercent(structured.primary.confidence)}`,
      ...structured.concurrent.map(
        c => `兼证：${c.name} ${confidencePercent(c.confidence)}`,
      ),
    )
  }
  lines.push('', '【免责声明】', result.disclaimer || DISCLAIMER_FALLBACK)
  return lines.filter(l => l !== '').join('\n')
}

/**
 * 单个证候卡片：证名 + 置信度 + 支持/矛盾证据（T4.1）。
 * 兼证与主证用同一组件渲染，只是标签不同（T4.2）——并存关系要在视觉上等权。
 */
function SyndromeCard({ kind, s }: { kind: string; s: SyndromeAssessment }) {
  return (
    <View className='chain-block'>
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
      {s.pathogenesis ? <Text className='plan-reason'>病机：{s.pathogenesis}</Text> : null}
    </View>
  )
}

/**
 * 诊断报告页：把 `/chat` 的 `steps` 与 `summary` 完整呈现。
 *
 * harness 无状态、不保存报告，因此本页只读取前端会话容器中的结果；
 * 刷新或直达会丢失，届时退回首页重新问诊。
 */
export default function ReportPage() {
  const [result, setResult] = useState<DiagnosisResult | null>(getResult())
  const [collapsed, setCollapsed] = useState<Record<number, boolean>>({})

  useEffect(() => {
    if (!getResult()) Taro.redirectTo({ url: '/pages/index/index' })
  }, [])

  if (!result) return <View className='report-page' />

  const toggle = (i: number) =>
    setCollapsed(prev => ({ ...prev, [i]: !prev[i] }))

  const copyEvidence = async () => {
    const text = buildEvidence(result)
    try {
      await Taro.setClipboardData({ data: text })
      Taro.showToast({ title: '存证信息已复制', icon: 'success' })
    } catch {
      // 某些环境（如开发者工具）不支持剪贴板：退回展示，用户可长按选择
      Taro.showModal({ title: '请手动复制', content: text.slice(0, 500), showCancel: false })
    }
  }

  const diff = result.structured?.differentiation
  const transformations = diff?.transformations ?? []

  return (
    <View className='report-page'>
      <ScrollView className='report-scroll' scrollY>
        {diff?.primary ? (
          <View className='card'>
            <View className='card-title'>
              辨证结构
              <Text className='sub-title'>　置信度与证据链</Text>
            </View>
            <SyndromeCard kind='主证' s={diff.primary} />
            {diff.concurrent.map(c => (
              <SyndromeCard key={c.slug} kind='兼证' s={c} />
            ))}
            {transformations.length ? (
              <View className='chain-block'>
                <Text className='chain-name'>传变提示</Text>
                <Text className='plan-reason'>{transformations.join('；')}</Text>
              </View>
            ) : null}
          </View>
        ) : null}

        <View className='card'>
          <View className='card-title'>辨证结论</View>
          <Text className='report-summary'>{result.summary}</Text>
        </View>

        <View className='card'>
          <View className='card-title'>分诊详情</View>
          {result.steps.map((s, i) => {
            const name = CAPABILITY_ZH[s.capability as HarnessCapability] || s.capability
            const open = !collapsed[i]
            return (
              <View key={`${s.capability}_${i}`} className='step-card'>
                <View className='step-head' onClick={() => toggle(i)}>
                  <Text className='step-name'>{name}</Text>
                  <Text className='step-fold'>{open ? '收起' : '展开'}</Text>
                </View>
                {open && <Text className='step-detail'>{s.text}</Text>}
              </View>
            )
          })}
        </View>

        <View className='card'>
          <View className='card-title'>
            存证与回查
            <Text className='sub-title'>　归档后可在「存证记录」中回查</Text>
          </View>
          {result.report_id ? (
            <View className='ev-row'>
              <Text className='ev-key'>报告编号</Text>
              <Text className='ev-val'>{result.report_id}</Text>
            </View>
          ) : (
            <Text className='rv-empty'>
              服务端未启用归档（HARNESS_STORE_DIR），当前报告仅存在于本页
            </Text>
          )}
          <View className='report-actions'>
            <View className='btn-ghost' onClick={copyEvidence}>复制存证信息</View>
            <View className='btn-ghost'
              onClick={() => Taro.navigateTo({ url: '/pages/reports/index' })}>
              存证记录
            </View>
          </View>
        </View>

        <View className='disclaimer-card'>
          <Text className='disclaimer-text'>{result.disclaimer || DISCLAIMER_FALLBACK}</Text>
        </View>

        <View className='report-actions'>
          <View className='btn-primary'
            onClick={() => Taro.navigateTo({ url: '/pages/consult/index' })}>
            继续追问
          </View>
          <View className='btn-ghost'
            onClick={() => Taro.reLaunch({ url: '/pages/index/index' })}>
            重新问诊
          </View>
        </View>
      </ScrollView>
    </View>
  )
}
