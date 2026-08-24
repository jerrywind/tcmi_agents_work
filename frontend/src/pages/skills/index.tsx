import { useState, useEffect } from 'react'
import Taro from '@tarojs/taro'
import { View, Text, Input } from '@tarojs/components'
import { getSkills, loadSkill, unloadSkill } from '../../services/api'
import type { SkillsList } from '../../types'
import './index.scss'

export default function SkillsPage() {
  const [data, setData] = useState<SkillsList | null>(null)
  const [name, setName] = useState('')
  const [loading, setLoading] = useState(false)

  const refresh = async () => {
    try {
      setData(await getSkills())
    } catch (e: any) {
      Taro.showToast({ title: e?.message || '加载失败', icon: 'none' })
    }
  }

  useEffect(() => { refresh() }, [])

  const doLoad = async () => {
    if (!name.trim() || loading) return
    setLoading(true)
    try {
      await loadSkill(name.trim())
      setName('')
      await refresh()
    } catch (e: any) {
      Taro.showToast({ title: e?.message || '装载失败', icon: 'none' })
    } finally {
      setLoading(false)
    }
  }

  const doUnload = async (n: string) => {
    try {
      await unloadSkill(n)
      await refresh()
    } catch (e: any) {
      Taro.showToast({ title: e?.message || '卸载失败', icon: 'none' })
    }
  }

  const skills = data?.skills || []

  return (
    <View className='skills-page'>
      <View className='card'>
        <View className='card-title'>装载技能（按名称）</View>
        <View className='skill-load-row'>
          <Input className='skill-input' placeholder='技能名，如 tcm-kb'
            value={name} onInput={e => setName(e.detail.value)} />
          <View className='btn-small' onClick={doLoad}>{loading ? '...' : '装载'}</View>
        </View>
        <Text className='skill-dir'>技能目录：{data?.skills_dir || '-'}</Text>
      </View>

      {skills.map(s => (
        <View className='card' key={s.name}>
          <View className='skill-head'>
            <Text className='skill-name'>{s.name}</Text>
            <Text className='skill-ver'>v{s.version}</Text>
            <View className='btn-small danger' onClick={() => doUnload(s.name)}>卸载</View>
          </View>
          <Text className='skill-desc'>{s.description}</Text>
          <View className='skill-tools'>
            {s.tools.map(t => (
              <View className='skill-tool' key={t.name}>
                <Text className='tool-name'>{t.name}</Text>
                <Text className='tool-cap'>{t.capability || '全部能力'}</Text>
                <Text className='tool-desc'>{t.description}</Text>
              </View>
            ))}
          </View>
        </View>
      ))}

      {skills.length === 0 && <Text className='empty-tip'>暂无已装载技能</Text>}
    </View>
  )
}
