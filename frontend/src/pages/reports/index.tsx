import { useEffect, useState } from 'react'
import Taro from '@tarojs/taro'
import { View, Text, ScrollView } from '@tarojs/components'
import { getReport, listReports } from '../../services/harness'
import { confidencePercent, formatDateTime } from '../../utils/format'
import { Markdown } from '../../utils/markdown'
import type { DiagnosisResult, ReportMeta, StoredReport } from '../../types'
import './index.scss'

/**
 * 存证记录页（T5.1）：列出服务端归档的报告，并可回查任意一份的完整快照。
 *
 * 用途有二：
 * - **复诊**：换设备/刷新后仍能找回上次的结论（前端 session 只在内存里）；
 * - **纠纷自证**：归档内容是「当时输入 + 当时输出」的完整快照，
 *   且落盘前已脱敏（T5.4），可安全留存。
 *
 * 服务端未启用持久化（`HARNESS_STORE_DIR` 未配置）时这里会是空列表，
 * 页面会明确说明原因，而不是让用户以为是「没有历史」。
 */
export default function ReportsPage() {
  const [reports, setReports] = useState<ReportMeta[]>([])
  const [enabled, setEnabled] = useState(true)
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(true)
  const [detail, setDetail] = useState<StoredReport | null>(null)
  const [opening, setOpening] = useState(false)

  const load = async () => {
    setLoading(true)
    try {
      const r = await listReports()
      setReports(r.reports || [])
      setEnabled(!!r.enabled)
      setError(r.hint || r.error || '')
    } catch (e: any) {
      setError(e?.message || '加载失败')
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => { load() }, [])

  // 回查是异步的：不加守卫，网络慢时连点会连发多个请求
  const open = async (id: string) => {
    if (opening) return
    setOpening(true)
    Taro.showLoading({ title: '加载中…' })
    try {
      const d = await getReport(id)
      setDetail(d)
    } catch (e: any) {
      Taro.showToast({ title: e?.message || '回查失败', icon: 'none' })
    } finally {
      Taro.hideLoading()
      setOpening(false)
    }
  }

  const summarize = (r: ReportMeta) => {
    const bits = [`${r.steps} 步`]
    if (r.primary_syndrome) bits.push(r.primary_syndrome)
    if (r.blocked) bits.push('已被安全门拦截')
    if (r.partial) bits.push('结果不完整')
    return bits.join(' · ')
  }

  // 详情页：直接复用报告页的渲染方式（只展示结论与免责声明，不做二次诊断）
  if (detail) return <ReportDetail report={detail} onBack={() => setDetail(null)} />

  return (
    <View className='reports-page'>
      <ScrollView className='reports-scroll' scrollY>
        <View className='card'>
          <View className='card-title'>
            存证记录
            <Text className='sub-title'>　服务端归档，落盘前已脱敏</Text>
          </View>
          {!enabled ? (
            <Text className='rv-empty'>
              服务端未启用报告持久化（HARNESS_STORE_DIR），无存证记录。
            </Text>
          ) : loading ? (
            <Text className='rv-empty'>加载中…</Text>
          ) : reports.length === 0 ? (
            <Text className='rv-empty'>{error || '暂无存证记录'}</Text>
          ) : (
            reports.map(r => (
              <View key={r.id} className='rep-row' onClick={() => open(r.id)}>
                <View className='rep-main'>
                  <Text className='rep-time'>{formatDateTime(r.created_at) || r.id}</Text>
                  <Text className='rep-sum'>{summarize(r)}</Text>
                  <Text className='rep-id'>{r.id}</Text>
                </View>
                <Text className='rep-open'>查看</Text>
              </View>
            ))
          )}
        </View>

        <View className='disclaimer-card'>
          <Text className='disclaimer-text'>
            归档内容仅含问诊输入（已脱敏）与本次结论，不含图片原图；
            如需彻底删除某份记录，请在服务端报告目录中移除对应 JSON 文件。
          </Text>
        </View>
      </ScrollView>
    </View>
  )
}

/** 单份归档报告的详情（回查视图） */
function ReportDetail({ report, onBack }: { report: StoredReport; onBack: () => void }) {
  const r: DiagnosisResult = report.result || ({} as DiagnosisResult)
  const structured = r.structured?.differentiation
  return (
    <View className='reports-page'>
      <ScrollView className='reports-scroll' scrollY>
        <View className='card'>
          <View className='card-title'>
            回查详情
            <Text className='sub-title'>　{formatDateTime(report.created_at)}</Text>
          </View>
          <View className='ev-row'>
            <Text className='ev-key'>报告编号</Text>
            <Text className='ev-val'>{report.id}</Text>
          </View>
          {structured?.primary ? (
            <View className='chain-block'>
              <View className='syndrome-row'>
                <Text className='chain-name'>{structured.primary.name}</Text>
                <Text className='syndrome-conf'>
                  {confidencePercent(structured.primary.confidence)}
                </Text>
              </View>
              {structured.concurrent.map(c => (
                <View key={c.slug} className='syndrome-row'>
                  <Text className='chain-name'>兼证：{c.name}</Text>
                  <Text className='syndrome-conf'>{confidencePercent(c.confidence)}</Text>
                </View>
              ))}
            </View>
          ) : null}
          {/* 与报告页用同一套渲染：回查看到的是同一份结论，观感必须一致。
              此前这里是纯 Text，Markdown 的标题与列表符号会原样露出来。 */}
          <View className='report-summary'><Markdown text={r.summary} /></View>
        </View>

        <View className='disclaimer-card'>
          <Text className='disclaimer-text'>{r.disclaimer || '本内容由 AI 生成，仅供健康参考。'}</Text>
        </View>

        <View className='report-actions'>
          <View className='btn-ghost' onClick={onBack}>返回列表</View>
        </View>
      </ScrollView>
    </View>
  )
}
