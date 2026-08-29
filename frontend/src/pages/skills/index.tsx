import { useEffect, useState } from 'react'
import Taro from '@tarojs/taro'
import { View, Text, Input } from '@tarojs/components'
import { callSkill, listSkills } from '../../services/harness'
import type { HarnessSkill } from '../../types'
import './index.scss'

/**
 * 技能一览与调试页。
 *
 * 注意：harness 的技能是**编译期内置注册**的 9 个，运行时**不能**装载/卸载
 * （旧 backend 的 `/api/skills/load`、`/api/skills/unload` 已不存在）。
 * 因此本页改为「浏览 + 直接调用调试」。
 */
export default function SkillsPage() {
  const [skills, setSkills] = useState<HarnessSkill[]>([])
  const [loading, setLoading] = useState(true)
  const [selected, setSelected] = useState<HarnessSkill | null>(null)
  const [args, setArgs] = useState('{}')
  const [output, setOutput] = useState('')
  const [busy, setBusy] = useState(false)

  const refresh = async () => {
    try {
      const r = await listSkills()
      setSkills(r.skills)
    } catch (e: any) {
      Taro.showToast({ title: e?.message || '加载失败', icon: 'none' })
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => { refresh() }, [])

  const pick = (s: HarnessSkill) => {
    setSelected(s)
    setOutput('')
    // 给一个能直接跑通的占位参数，减少手工拼 JSON 的摩擦
    if (/^tcm-(vision|auscultation|inquiry|palpation|reference|safety)$/.test(s.name)) {
      setArgs(JSON.stringify({ text: '舌红苔黄腻，口苦口臭' }, null, 2))
    } else if (s.name === 'tcm-diet') {
      setArgs(JSON.stringify({ syndrome: '脾胃湿热' }, null, 2))
    } else {
      setArgs(JSON.stringify({ query: '脾胃湿热' }, null, 2))
    }
  }

  const run = async () => {
    if (!selected || busy) return
    let parsed: Record<string, any> = {}
    try {
      parsed = JSON.parse(args || '{}')
    } catch {
      Taro.showToast({ title: '参数必须是合法 JSON', icon: 'none' })
      return
    }
    setBusy(true)
    setOutput('调用中…')
    try {
      const r = await callSkill(selected.name, parsed)
      setOutput(typeof r === 'string' ? r : JSON.stringify(r, null, 2))
    } catch (e: any) {
      setOutput(`调用失败：${e?.message || String(e)}`)
    } finally {
      setBusy(false)
    }
  }

  return (
    <View className='skills-page'>
      <View className='card'>
        <View className='card-title'>技能清单（{skills.length}）</View>
        <Text className='skill-tip'>
          技能为编译期内置注册，不支持运行时装载/卸载。
        </Text>
      </View>

      {skills.map(s => (
        <View key={s.name}
          className={`card skill-card ${selected?.name === s.name ? 'selected' : ''}`}
          onClick={() => pick(s)}>
          <View className='skill-head'>
            <Text className='skill-name'>{s.name}</Text>
            <Text className='skill-owner'>{s.owner}</Text>
          </View>
          <Text className='skill-desc'>{s.description}</Text>
        </View>
      ))}

      {skills.length === 0 && !loading && (
        <Text className='empty-tip'>未能获取技能列表，请确认 harness 已启动</Text>
      )}

      {selected && (
        <View className='card'>
          <View className='card-title'>调用 {selected.name}</View>
          <Input className='skill-input' placeholder='JSON 参数'
            value={args} onInput={e => setArgs(e.detail.value)} />
          <View className={`btn-small ${busy ? 'disabled' : ''}`} onClick={run}>
            {busy ? '调用中…' : '执行'}
          </View>
          {output ? <Text className='skill-output'>{output}</Text> : null}
        </View>
      )}
    </View>
  )
}
