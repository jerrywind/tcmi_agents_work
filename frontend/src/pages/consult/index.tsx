import { useEffect, useRef, useState } from 'react'
import Taro, { useRouter } from '@tarojs/taro'
import { View, Text, Input, ScrollView } from '@tarojs/components'
import { answerQuestion, getState, getStream, postPpg, startConsultation } from '../../services/api'
import type { ConsultState } from '../../types'
import './index.scss'

const PHASES = ['望', '闻', '问', '切', '治']

const PROFILE_LABEL: Record<string, string> = {
  normal: '常脉', slippery: '滑脉', choppy: '涩脉', weak: '弱脉', taut: '弦脉',
}

interface LocalSeg {
  seq: number
  role: 'agent' | 'user' | 'system'
  type: 'text' | 'question' | 'report' | 'alert'
  content: string
  done: boolean
}

export default function ConsultPage() {
  const router = useRouter()
  const cid = router.params.id || ''
  const [state, setState] = useState<ConsultState | null>(null)
  const [freeText, setFreeText] = useState('')
  const [busy, setBusy] = useState(false)
  const [streamSegs, setStreamSegs] = useState<Record<number, LocalSeg>>({})
  const started = useRef(false)
  const polling = useRef(false)
  const afterRef = useRef(0)

  useEffect(() => {
    if (!cid || started.current) return
    started.current = true
    ;(async () => {
      try {
        const s0 = await getState(cid)
        const s = s0.status === 'created' ? await startConsultation(cid) : s0
        setState(s)
        if (s.task_id) startPolling(s.task_id)
        else maybeFinish(s)
      } catch (e: any) {
        Taro.showToast({ title: e?.message || '加载失败', icon: 'none' })
      }
    })()
  }, [cid])

  // 实时流式轮询：AI 边说边渲染
  const startPolling = (taskId: string) => {
    if (polling.current) return
    polling.current = true
    setBusy(true)
    const loop = async () => {
      try {
        const { task, segs } = await getStream(cid, afterRef.current)
        if (segs.length) {
          setStreamSegs(prev => {
            const next = { ...prev }
            for (const sg of segs) {
              next[sg.seq] = sg
              afterRef.current = Math.max(afterRef.current, sg.seq)
            }
            return next
          })
        }
        if (task === 'done') {
          const s = await getState(cid)
          setState(s)
          setStreamSegs({})
          maybeFinish(s)
          polling.current = false
          setBusy(false)
          return
        }
        if (task === 'error') {
          Taro.showToast({ title: '问诊引擎异常', icon: 'none' })
          polling.current = false
          setBusy(false)
          return
        }
        setTimeout(loop, 600)
      } catch {
        setTimeout(loop, 1000)
      }
    }
    loop()
  }

  const maybeFinish = (s: ConsultState) => {
    if (s.status === 'finished' || s.status === 'referred') {
      setTimeout(() => {
        Taro.redirectTo({ url: `/pages/report/index?id=${s.id}` })
      }, 1200)
    }
  }

  const submit = async (value: string, text: string) => {
    if (!state?.question || busy) return
    setBusy(true)
    try {
      const s = await answerQuestion(cid, state.question.id, value, text)
      setState(s)
      setFreeText('')
      afterRef.current = 0
      setStreamSegs({})
      if (s.task_id) startPolling(s.task_id)
      else maybeFinish(s)
    } catch (e: any) {
      Taro.showToast({ title: e?.message || '提交失败', icon: 'none' })
      setBusy(false)
    }
  }

  const simulatePulse = async (profile: string) => {
    if (busy) return
    setBusy(true)
    Taro.showLoading({ title: '采样脉象中...' })
    try {
      const s = await postPpg(cid, { simulate: true, profile, rate_bpm: 72 })
      setState(s)
      Taro.showToast({ title: '脉象已采集', icon: 'success' })
    } catch (e: any) {
      Taro.showToast({ title: e?.message || '采集失败', icon: 'none' })
    } finally {
      setBusy(false)
      Taro.hideLoading()
    }
  }

  if (!state) return <View className='consult-page' />

  const top3 = state.hypotheses.slice(0, 3).filter(h => h.confidence > 0)
  const ppg = state.ppg
  // 合并：已落库 messages + 还未 done 的流式段（避免重复显示 done 段）
  const liveSegs = Object.values(streamSegs).filter(s => !s.done)

  return (
    <View className='consult-page'>
      {/* 进度区：望闻问切 + 候选证候收窄 */}
      <View className='progress-bar'>
        <View className='phases'>
          {PHASES.map(p => <Text key={p} className='phase active'>{p}</Text>)}
          <Text className='round-info'>第 {state.round} 轮</Text>
        </View>
        {top3.length > 0 && (
          <View className='hyps'>
            {top3.map(h => (
              <View key={h.name} className='hyp-chip'>
                <Text className='hyp-name'>{h.name}</Text>
                <View className='hyp-track'>
                  <View className='hyp-fill'
                    style={{ width: `${Math.round(h.confidence * 100)}%` }} />
                </View>
                <Text className='hyp-pct'>{Math.round(h.confidence * 100)}%</Text>
              </View>
            ))}
          </View>
        )}
      </View>

      {/* 脉象采集卡片（PPG 模拟/硬件接入） */}
      <View className='ppg-card'>
        <View className='ppg-head'>
          <Text className='ppg-title'>脉象（切诊）</Text>
          <Text className='ppg-tip'>接 PPG 手环或模拟采样</Text>
        </View>
        {ppg ? (
          <View className='ppg-result'>
            <Text className='ppg-main'>{ppg.depth}脉{ppg.force} · {ppg.shape}</Text>
            <Text className='ppg-sub'>{ppg.notes}</Text>
            <View className='ppg-tags'>
              <Text className='ppg-tag'>脉率 {ppg.rate_bpm} 次/分</Text>
              <Text className='ppg-tag'>节律 {ppg.rhythm}</Text>
              <Text className='ppg-tag'>信噪 {Math.round(ppg.signal_quality * 100)}%</Text>
            </View>
          </View>
        ) : (
          <Text className='ppg-empty'>未采集脉象</Text>
        )}
        <View className='ppg-actions'>
          {['normal', 'slippery', 'choppy', 'weak', 'taut'].map(p => (
            <View key={p} className={`ppg-btn ${busy ? 'disabled' : ''}`}
              onClick={() => simulatePulse(p)}>
              {PROFILE_LABEL[p]}
            </View>
          ))}
        </View>
      </View>

      {/* 聊天流 */}
      <ScrollView className='chat' scrollY scrollIntoView='chat-bottom'>
        {state.messages.map(m => (
          <View key={m.id}
            className={`bubble-row ${m.role === 'user' ? 'right' : 'left'}`}>
            <View className={`bubble ${m.role} ${m.type}`}>
              <Text>{m.content}</Text>
            </View>
          </View>
        ))}
        {liveSegs.map(s => (
          <View key={`s_${s.seq}`}
            className={`bubble-row ${s.role === 'user' ? 'right' : 'left'}`}>
            <View className={`bubble ${s.role} ${s.type}`}>
              <Text>{s.content}{busy && !s.done ? '▌' : ''}</Text>
            </View>
          </View>
        ))}
        {(state.status === 'finished' || state.status === 'referred') && (
          <View className='bubble-row left'>
            <View className='bubble agent'>
              <Text>正在为您生成报告，即将跳转...</Text>
            </View>
          </View>
        )}
        {state.status === 'planning' && (
          <View className='bubble-row left'>
            <View className='bubble agent'>
              <Text>AI 医师正在为您制定诊疗方案…</Text>
            </View>
          </View>
        )}
        <View id='chat-bottom' style={{ height: '20px' }} />
      </ScrollView>

      {/* 答题区（辨证追问 与 诊疗方案个性化追问 复用） */}
      {['waiting_answer', 'treatment_qa'].includes(state.status) && state.question && (
        <View className='answer-panel'>
          <View className='options'>
            {state.question.options.map(o => (
              <View key={o.value} className={`option ${busy ? 'disabled' : ''}`}
                onClick={() => submit(o.value, '')}>
                {o.label}
              </View>
            ))}
          </View>
          {state.question.allow_free_text && (
            <View className='free-row'>
              <Input className='free-input' placeholder='或用文字补充描述...'
                value={freeText} onInput={e => setFreeText(e.detail.value)}
                confirmType='send'
                onConfirm={() => freeText.trim() && submit('', freeText.trim())} />
              <View className={`send-btn ${freeText.trim() && !busy ? '' : 'disabled'}`}
                onClick={() => freeText.trim() && submit('', freeText.trim())}>
                发送
              </View>
            </View>
          )}
        </View>
      )}
    </View>
  )
}
