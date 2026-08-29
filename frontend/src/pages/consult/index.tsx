import { useEffect, useState } from 'react'
import Taro from '@tarojs/taro'
import { View, Text, Input, ScrollView } from '@tarojs/components'
import { CAPABILITY_ZH, chat } from '../../services/harness'
import {
  getMessages, getPayload, getResult, pushMessage, setResult,
} from '../../services/session'
import type { DiagnosisResult, HarnessCapability } from '../../types'
import './index.scss'

/**
 * 问诊页：展示 `/chat` 返回的各步结果，并支持多轮追问。
 *
 * harness 没有服务端多轮循环——一次 `/chat` 会把 routing.yaml 中的
 * 全部步骤串行跑完。所谓「多轮」由本页实现：把用户输入追加进 messages
 * 后**重新**调用 `/chat`，模型因此能看到完整历史。
 */
export default function ConsultPage() {
  const [result, setLocalResult] = useState<DiagnosisResult | null>(getResult())
  const [activeIdx, setActiveIdx] = useState(0)
  const [input, setInput] = useState('')
  const [busy, setBusy] = useState(false)

  // 直达本页（没有问诊结果）时退回首页，避免出现空白页
  useEffect(() => {
    if (!getResult()) Taro.redirectTo({ url: '/pages/index/index' })
  }, [])

  const ask = async () => {
    const text = input.trim()
    if (!text || busy) return
    setBusy(true)
    setInput('')
    Taro.showLoading({ title: '思考中…' })
    try {
      pushMessage({ role: 'user', content: text })
      const r = await chat(getMessages(), getPayload())
      setResult(r)
      setLocalResult(r)
      setActiveIdx(0)
      Taro.hideLoading()
    } catch (e: any) {
      Taro.hideLoading()
      Taro.showToast({ title: e?.message || '追问失败', icon: 'none' })
    } finally {
      setBusy(false)
    }
  }

  if (!result) return <View className='consult-page' />

  const steps = result.steps
  const current = steps[Math.min(activeIdx, steps.length - 1)]

  return (
    <View className='consult-page'>
      {/* 步骤导航：望 → 闻 → 问 → 切 → 辨证 → 安全门 → 治疗 */}
      <View className='progress-bar'>
        <ScrollView className='phases' scrollX>
          {steps.map((s, i) => (
            <View key={`${s.capability}_${i}`}
              className={`phase ${i === activeIdx ? 'active' : ''}`}
              onClick={() => setActiveIdx(i)}>
              <Text>{CAPABILITY_ZH[s.capability as HarnessCapability] || s.capability}</Text>
            </View>
          ))}
        </ScrollView>
      </View>

      <ScrollView className='step-body' scrollY>
        <Text className='step-title'>
          {CAPABILITY_ZH[current?.capability as HarnessCapability] || current?.capability || ''}
        </Text>
        <Text className='step-text'>{current?.text || '（无输出）'}</Text>
      </ScrollView>

      {/* 追问区：追加到 messages 后重新 /chat */}
      <View className='answer-panel'>
        <View className='free-row'>
          <Input className='free-input' placeholder='补充症状，或追问上面任意一步…'
            value={input} onInput={e => setInput(e.detail.value)}
            confirmType='send' onConfirm={ask} />
          <View className={`send-btn ${input.trim() && !busy ? '' : 'disabled'}`}
            onClick={ask}>
            {busy ? '…' : '发送'}
          </View>
        </View>
        <View className='report-entry'
          onClick={() => Taro.navigateTo({ url: '/pages/report/index' })}>
          查看完整报告
        </View>
      </View>
    </View>
  )
}
