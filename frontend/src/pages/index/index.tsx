import { useEffect, useState } from 'react'
import Taro, { useRouter } from '@tarojs/taro'
import { View, Text, Input, Textarea, Picker } from '@tarojs/components'
import { chat } from '../../services/harness'
import { getMember } from '../../services/members'
import { getMessages, pushMessage, setResult, startSession } from '../../services/session'
import type { PatientProfile } from '../../types'
import './index.scss'

const GENDERS = ['男', '女', '未知']
const GENDER_IDX: Record<string, number> = { 男: 0, 女: 1, 未知: 2 }

export default function ProfilePage() {
  const router = useRouter()
  const mid = router.params.mid || ''

  const [region, setRegion] = useState('')
  const [height, setHeight] = useState('')
  const [weight, setWeight] = useState('')
  const [age, setAge] = useState('')
  const [genderIdx, setGenderIdx] = useState(2)
  const [heartRate, setHeartRate] = useState('')
  const [complaint, setComplaint] = useState('')
  const [submitting, setSubmitting] = useState(false)

  // 从家庭档案进入时，用本地成员档案预填
  useEffect(() => {
    if (!mid) return
    const m = getMember(mid)
    if (!m) return
    setRegion(m.patient.region || '')
    setHeight(m.patient.height_cm ? String(m.patient.height_cm) : '')
    setWeight(m.patient.weight_kg ? String(m.patient.weight_kg) : '')
    setAge(m.patient.age ? String(m.patient.age) : '')
    setGenderIdx(GENDER_IDX[m.patient.gender] ?? 2)
  }, [mid])

  const canSubmit = complaint.trim().length >= 5 && !submitting

  const submit = async () => {
    if (!canSubmit) return
    setSubmitting(true)
    Taro.showLoading({ title: '问诊中，请稍候…' })
    try {
      const profile: PatientProfile = {
        region: region.trim() || undefined,
        height_cm: parseFloat(height) || undefined,
        weight_kg: parseFloat(weight) || undefined,
        age: parseInt(age, 10) || undefined,
        gender: GENDERS[genderIdx],
        heart_rate: heartRate ? parseFloat(heartRate) : undefined,
      }

      // harness 无状态：由前端持有 messages，每次携带完整对话历史
      startSession(profile)
      pushMessage({ role: 'user', content: complaint.trim() })

      const r = await chat(getMessages(), profile)
      setResult(r)

      Taro.hideLoading()
      Taro.navigateTo({ url: '/pages/consult/index' })
    } catch (e: any) {
      Taro.hideLoading()
      Taro.showToast({
        title: e?.message || '问诊失败，请确认后端已启动且 LLM 可用',
        icon: 'none',
      })
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <View className='profile-page'>
      <View className='card'>
        <View className='card-title'>基本信息</View>
        <View className='form-row'>
          <Text className='form-label'>常住地</Text>
          <Input className='form-input' placeholder='如：广州' value={region}
            onInput={e => setRegion(e.detail.value)} />
        </View>
        <View className='form-row'>
          <Text className='form-label'>身高(cm)</Text>
          <Input className='form-input' type='digit' placeholder='170' value={height}
            onInput={e => setHeight(e.detail.value)} />
        </View>
        <View className='form-row'>
          <Text className='form-label'>体重(kg)</Text>
          <Input className='form-input' type='digit' placeholder='65' value={weight}
            onInput={e => setWeight(e.detail.value)} />
        </View>
        <View className='form-row'>
          <Text className='form-label'>年龄</Text>
          <Input className='form-input' type='number' placeholder='30' value={age}
            onInput={e => setAge(e.detail.value)} />
        </View>
        <Picker mode='selector' range={GENDERS} value={genderIdx}
          onChange={e => setGenderIdx(Number(e.detail.value))}>
          <View className='form-row'>
            <Text className='form-label'>性别</Text>
            <Text className='form-input'>{GENDERS[genderIdx]}</Text>
          </View>
        </Picker>
        <View className='form-row'>
          <Text className='form-label'>静息心率</Text>
          <Input className='form-input' type='number' placeholder='选填，次/分'
            value={heartRate} onInput={e => setHeartRate(e.detail.value)} />
        </View>
      </View>

      <View className='card'>
        <View className='card-title'>病情自述 *</View>
        <Textarea className='complaint-input' maxlength={2000}
          placeholder='请描述您的不适症状、持续时间、诱因等（不少于 5 个字）'
          value={complaint} onInput={e => setComplaint(e.detail.value)} />
        <Text className='card-note'>
          建议一并写上舌象（如「舌红苔黄腻」）、二便、寒热、睡眠等信息，
          四诊 Agent 会据此直接取证。
        </Text>
      </View>

      <View className='submit-wrap'>
        <View className={`btn-primary ${canSubmit ? '' : 'disabled'}`} onClick={submit}>
          {submitting ? '问诊中…' : '开始问诊'}
        </View>
        <Text className='disclaimer'>本服务由 AI 提供健康参考，不构成医疗诊断</Text>
      </View>

      <View className='skills-entry' onClick={() => Taro.navigateTo({ url: '/pages/family/index' })}>
        家庭档案 / 成员管理（仅存本机）
      </View>
      <View className='skills-entry' onClick={() => Taro.navigateTo({ url: '/pages/skills/index' })}>
        技能 / SKILL 一览
      </View>
    </View>
  )
}
