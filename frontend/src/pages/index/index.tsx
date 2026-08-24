import { useEffect, useState } from 'react'
import Taro, { useRouter } from '@tarojs/taro'
import { View, Text, Input, Textarea, Picker, Image } from '@tarojs/components'
import { createConsultation, getFamily, uploadImage } from '../../services/api'
import './index.scss'

const GENDERS = ['男', '女', '未知']

interface LocalImage { type: 'tongue' | 'face' | 'lesion' | 'palm_left' | 'palm_right'; path: string }

const GENDER_IDX: Record<string, number> = { '男': 0, '女': 1, '未知': 2 }

export default function ProfilePage() {
  const router = useRouter()
  const fid = router.params.fid || ''
  const mid = router.params.mid || ''
  const [region, setRegion] = useState('')
  const [height, setHeight] = useState('')
  const [weight, setWeight] = useState('')
  const [age, setAge] = useState('')
  const [genderIdx, setGenderIdx] = useState(2)
  const [heartRate, setHeartRate] = useState('')
  const [complaint, setComplaint] = useState('')
  const [images, setImages] = useState<LocalImage[]>([])
  const [submitting, setSubmitting] = useState(false)

  const pickImage = async (type: LocalImage['type']) => {
    try {
      const res = await Taro.chooseImage({ count: 1, sizeType: ['compressed'] })
      if (res.tempFilePaths?.length) {
        setImages(prev => [...prev.filter(i => i.type !== type),
          { type, path: res.tempFilePaths[0] }])
      }
    } catch { /* 用户取消 */ }
  }

  const imgOf = (type: LocalImage['type']) => images.find(i => i.type === type)

  // 从家庭档案进入时，预填成员体质档案
  useEffect(() => {
    if (!fid || !mid) return
    ;(async () => {
      try {
        const f = await getFamily(fid)
        const m = f.members.find(x => x.id === mid)
        if (!m) return
        setRegion(m.patient.region || '')
        setHeight(m.patient.height_cm ? String(m.patient.height_cm) : '')
        setWeight(m.patient.weight_kg ? String(m.patient.weight_kg) : '')
        setAge(m.patient.age ? String(m.patient.age) : '')
        setGenderIdx(GENDER_IDX[m.patient.gender] ?? 2)
      } catch { /* 忽略预填错误 */ }
    })()
  }, [fid, mid])

  const canSubmit = complaint.trim().length >= 5 && !submitting

  const submit = async () => {
    if (!canSubmit) return
    setSubmitting(true)
    Taro.showLoading({ title: '创建档案中...' })
    try {
      const state = await createConsultation({
        region,
        height_cm: parseFloat(height) || 0,
        weight_kg: parseFloat(weight) || 0,
        age: parseInt(age) || 0,
        gender: GENDERS[genderIdx]
      }, complaint.trim(), heartRate ? { heart_rate: parseFloat(heartRate) } : {},
        fid, mid)

      for (const img of images) {
        await uploadImage(state.id, img.type, img.path)
      }
      Taro.hideLoading()
      Taro.navigateTo({ url: `/pages/consult/index?id=${state.id}` })
    } catch (e: any) {
      Taro.hideLoading()
      Taro.showToast({ title: e?.message || '创建失败', icon: 'none' })
    } finally {
      setSubmitting(false)
    }
  }

  const renderUploader = (type: LocalImage['type'], label: string, tip: string) => {
    const img = imgOf(type)
    return (
      <View className='uploader' onClick={() => pickImage(type)}>
        {img
          ? <Image className='uploader-img' src={img.path} mode='aspectFill' />
          : <View className='uploader-plus'>＋</View>}
        <Text className='uploader-label'>{label}</Text>
        <Text className='uploader-tip'>{tip}</Text>
      </View>
    )
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
        <Textarea className='complaint-input' maxlength={500}
          placeholder='请描述您的不适症状、持续时间、诱因等（不少于5个字）'
          value={complaint} onInput={e => setComplaint(e.detail.value)} />
      </View>

      <View className='card'>
        <View className='card-title'>照片采集（选填）</View>
        <View className='uploader-row'>
          {renderUploader('tongue', '舌象', '自然光下伸舌')}
          {renderUploader('face', '面相', '正面免冠')}
          {renderUploader('lesion', '患处', '如有皮损等')}
        </View>
        <View className='uploader-row'>
          {renderUploader('palm_left', '左手掌纹', '掌心朝上平铺')}
          {renderUploader('palm_right', '右手掌纹', '掌心朝上平铺')}
        </View>
        <Text className='card-note'>中医手诊：掌色、掌纹、指形可辅助判断气血盛衰与脏腑状态，建议双手都拍。</Text>
      </View>

      <View className='submit-wrap'>
        <View className={`btn-primary ${canSubmit ? '' : 'disabled'}`} onClick={submit}>
          开始问诊
        </View>
        <Text className='disclaimer'>本服务由 AI 提供健康参考，不构成医疗诊断</Text>
      </View>

      <View className='skills-entry' onClick={() => Taro.navigateTo({ url: '/pages/family/index' })}>
        家庭档案 / 家庭成员管理
      </View>
      <View className='skills-entry' onClick={() => Taro.navigateTo({ url: '/pages/skills/index' })}>
        管理技能 / SKILL
      </View>
    </View>
  )
}
