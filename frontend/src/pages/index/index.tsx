import { useEffect, useState } from 'react'
import Taro, { useRouter } from '@tarojs/taro'
import { View, Text, Input, Textarea } from '@tarojs/components'
import { getMember } from '../../services/members'
import { getProfile, startSession } from '../../services/session'
import {
  EMPTY_PROFILE_FORM, GENDER_OPTIONS, buildProfile, toProfileForm, todayISO,
  validateProfileForm,
} from '../../utils/profile'
import type { ProfileForm } from '../../utils/profile'
import BirthDatePicker from '../../components/BirthDatePicker'
import './index.scss'

const BIRTH_DATE_START = '1900-01-01'

/**
 * 档案页：**只收档案，不收病情自述**。
 *
 * 主诉属于这一次"得了什么病"，档案属于"这个人是谁"，两者生命周期不同：
 * 档案可以留着下次复用，主诉每次问诊都重填。混在一页上，用户填完主诉
 * 才发现档案填错了，返回来主诉也没了。
 * 填完点「下一步」进问诊页（`pages/consult`），在那里描述病情。
 */
export default function ProfilePage() {
  const router = useRouter()
  const mid = router.params.mid || ''

  // 从问诊页返回改档案时，用上次填的档案回填——页面栈回退会重新挂载，
  // 不回填就得从头再填一遍
  const [form, setForm] = useState<ProfileForm>(() => {
    const p = getProfile()
    return p ? toProfileForm(p) : { ...EMPTY_PROFILE_FORM }
  })
  // 出生日期选择器的最晚可选日期（今天）；最早在组件里写死 1900
  const [maxDate] = useState(todayISO())

  // 从家庭档案进入时，用本地成员档案覆盖预填
  useEffect(() => {
    if (!mid) return
    const m = getMember(mid)
    if (!m) return
    setForm({ ...toProfileForm(m.patient), name: m.name })
  }, [mid])

  const patch = (p: Partial<ProfileForm>) => setForm({ ...form, ...p })

  const next = () => {
    const err = validateProfileForm(form)
    if (err) {
      Taro.showToast({ title: err, icon: 'none' })
      return
    }
    startSession(buildProfile(form))
    Taro.navigateTo({ url: '/pages/consult/index' })
  }

  return (
    <View className='profile-page'>
      <View className='card'>
        <View className='card-title'>基本信息</View>
        <View className='form-row'>
          <Text className='form-label'>姓名（选填）</Text>
          <Input className='form-input' placeholder='如：张三'
            value={form.name}
            onInput={e => patch({ name: e.detail.value })} />
        </View>
        <BirthDatePicker
          value={form.birthDate}
          start={BIRTH_DATE_START}
          end={maxDate}
          onChange={v => patch({ birthDate: v })}
        />
        <View className='form-row'>
          <Text className='form-label'>性别</Text>
          <View className='gender-group'>
            {GENDER_OPTIONS.map(g => (
              <View key={g} className={`gender-chip ${form.gender === g ? 'active' : ''}`}
                onClick={() => patch({ gender: g })}>
                {g}
              </View>
            ))}
          </View>
        </View>
        <View className='form-row'>
          <Text className='form-label'>常住地</Text>
          <Input className='form-input' placeholder='如：广州'
            value={form.region}
            onInput={e => patch({ region: e.detail.value })} />
        </View>
        <View className='form-row col'>
          <Text className='form-label'>既往病史（选填）</Text>
          <Textarea className='history-input' maxlength={500}
            placeholder='慢病、过敏史、手术史、长期用药等，如：高血压 5 年，青霉素过敏'
            value={form.history}
            onInput={e => patch({ history: e.detail.value })} />
        </View>
      </View>

      <View className='submit-wrap'>
        <View className='btn-primary' onClick={next}>下一步：描述病情</View>
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
