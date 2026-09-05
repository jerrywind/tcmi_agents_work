import { useEffect, useState } from 'react'
import Taro from '@tarojs/taro'
import { View, Text, Input, Textarea, ScrollView, Image } from '@tarojs/components'
import { CAPABILITY_ZH, chat } from '../../services/harness'
import {
  advanceRound, getMessages, getPayload, getProfile, getResult, pushMessage, resetMessages,
  setResult,
} from '../../services/session'
import { nearHints } from '../../utils/differentiation'
import { buildOpeningMessages, buildResidenceLine, describeProfile, RESIDENCE_DURATION_OPTIONS } from '../../utils/profile'
import { Markdown } from '../../utils/markdown'
import { chooseImageAsDataURL } from '../../utils/image'
import type { DiagnosisResult, HarnessCapability } from '../../types'
import './index.scss'

/**
 * 单张体征图片的选择/预览/清除槽。
 *
 * 舌苔与手相共用同一个组件，只是 label 不同。选中后展示缩略图与「重拍」按钮，
 * 点击缩略图可重新选择，右上角「×」清除。
 */
function ImageSlot({
  label, image, onPick, onClear,
}: {
  label: string
  image: string
  onPick: () => void
  onClear: () => void
}) {
  return (
    <View className='image-slot'>
      <Text className='image-slot-label'>{label}</Text>
      {image ? (
        <View className='image-preview'>
          <Image className='image-thumb' src={image} mode='aspectFill' onClick={onPick} />
          <Text className='image-clear' onClick={onClear}>×</Text>
          <Text className='image-retake' onClick={onPick}>重拍</Text>
        </View>
      ) : (
        <View className='image-pick' onClick={onPick}>
          <Text className='image-pick-plus'>+</Text>
          <Text className='image-pick-text'>上传照片</Text>
        </View>
      )}
    </View>
  )
}

/**
 * 问诊页：先收主诉，再展示 `/chat` 返回的各步结果，并支持多轮追问。
 *
 * 档案在 `pages/index` 收，主诉在这里收——主诉属于这一次「得了什么病」，
 * 档案属于「这个人是谁」，混在一页上，填完主诉才发现档案错了，回头主诉也没了。
 *
 * harness 没有服务端多轮循环——一次 `/chat` 会把 routing.yaml 中的
 * 全部步骤串行跑完。所谓「多轮」由本页实现：把用户输入追加进 messages
 * 后**重新**调用 `/chat`，模型因此能看到完整历史。
 */
export default function ConsultPage() {
  const [profile] = useState(getProfile())
  const [result, setLocalResult] = useState<DiagnosisResult | null>(getResult())
  const [activeIdx, setActiveIdx] = useState(0)
  const [complaint, setComplaint] = useState('')
  const [input, setInput] = useState('')
  const [busy, setBusy] = useState(false)
  // 当前居住地采集：近期所在地（文本）+ 居住时长（点选标签，均可不填）
  const [residence, setResidence] = useState('')
  const [residenceDuration, setResidenceDuration] = useState('')
  // 舌苔 / 左手手相 / 右手手相图片采集：存 data URL，随 `/chat` 的 payload.images 发后端做望诊。
  // 手相分左右手两个独立槽位，且都可不提供（模型靠纹理/色泽望诊，缺图则以其它信息推断）。
  const [tongueImage, setTongueImage] = useState('')
  const [palmLeftImage, setPalmLeftImage] = useState('')
  const [palmRightImage, setPalmRightImage] = useState('')

  /** 选图：把结果写进对应 slot；用户取消则返回 null，UI 不变。 */
  const pickImage = async (setter: (v: string) => void) => {
    const url = await chooseImageAsDataURL()
    if (url) setter(url)
  }

  /** 把已采集的图片整理成后端约定的 `images` 数组（无图则为空）。 */
  const collectedImages = () => {
    const imgs: { kind: string; data_url: string }[] = []
    if (tongueImage) imgs.push({ kind: 'tongue', data_url: tongueImage })
    if (palmLeftImage) imgs.push({ kind: 'palm_left', data_url: palmLeftImage })
    if (palmRightImage) imgs.push({ kind: 'palm_right', data_url: palmRightImage })
    return imgs
  }

  // 直达本页却没有档案：payload 无从构造，退回档案页
  useEffect(() => {
    if (!profile) Taro.redirectTo({ url: '/pages/index/index' })
  }, [profile])

  /** 首诊：主诉 + 档案里的既往病史一起发出去。首诊仍是第 1 轮，不推进轮次。 */
  const startDiagnosis = async () => {
    const text = complaint.trim()
    if (text.length < 5 || busy || !profile) return
    setBusy(true)
    setComplaint('')
    Taro.showLoading({ title: '问诊中，请稍候…' })
    try {
      // 既往病史独立成条、排在主诉之前（理由见 buildOpeningMessages 注释）
      const opening = buildOpeningMessages(text, profile.history || '')
      // 当前居住地作为上下文，插到主诉之前（与既往病史同级）；缺则整条不注入
      const resLine = buildResidenceLine(residence, residenceDuration)
      if (resLine) opening.splice(opening.length - 1, 0, { role: 'user', content: resLine })
      opening.forEach(pushMessage)
      const r = await chat(getMessages(), {
        ...getPayload(),
        images: collectedImages(),
      })
      setResult(r)
      setLocalResult(r)
      setActiveIdx(0)
      Taro.hideLoading()
    } catch (e: any) {
      // 首诊失败要把已经推进去的主诉撤回来，否则重试一次就多发一遍
      resetMessages()
      setComplaint(text)
      Taro.hideLoading()
      Taro.showToast({
        title: e?.message || '问诊失败，请确认后端已启动且 LLM 可用',
        icon: 'none',
      })
    } finally {
      setBusy(false)
    }
  }

  const ask = async () => {
    const text = input.trim()
    if (!text || busy) return
    setBusy(true)
    setInput('')
    Taro.showLoading({ title: '思考中…' })
    try {
      pushMessage({ role: 'user', content: text })
      // 在已有结论之上补充 → 轮次 +1，后端的「达到上限强制放行」才可能触发
      advanceRound()
      const r = await chat(getMessages(), {
        ...getPayload(),
        images: collectedImages(),
      })
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

  // 首诊前：只让用户描述这一次的病情
  if (!result) {
    const canStart = complaint.trim().length >= 5 && !busy && !!profile
    return (
      <View className='consult-page'>
        <View className='profile-summary'>
          <View className='summary-main'>
            <Text className='summary-label'>问诊对象</Text>
            <Text className='summary-text'>
              {profile ? (describeProfile(profile) || '档案待完善') : ''}
            </Text>
          </View>
          <Text className='summary-edit' onClick={() => Taro.navigateBack()}>修改档案</Text>
        </View>

        <View className='card'>
          <View className='card-title'>病情自述 *</View>
          <Textarea className='complaint-input' maxlength={2000}
            placeholder='请描述您的不适症状、持续时间、诱因等（不少于 5 个字）'
            value={complaint} onInput={e => setComplaint(e.detail.value)} />
          <Text className='card-note'>
            舌象、左右手手相请在下方的「体征图片采集」里上传照片供望诊参考（手相均可不提供）；
            脉象无需提供，由系统结合其它信息推断。
          </Text>
        </View>

        {/* 当前居住地（选填）：近期所在地 + 居住时长，作为辨证上下文（水土不服 / 时令外邪）。
            居住时长用点选标签而非 Picker：Picker 点开直接点「确定」会静默写入定位值（出生日期那条踩过）。 */}
        <View className='card'>
          <View className='card-title'>当前居住地（选填）</View>
          <Text className='card-note'>
            填写近期所在地与居住时长，有助于判断是否新到异地（水土不服、时令外邪等）。
          </Text>
          <Input className='residence-input' placeholder='如：广州'
            value={residence} onInput={e => setResidence(e.detail.value)} />
          <Text className='card-subnote'>居住时长</Text>
          <View className='duration-group'>
            {RESIDENCE_DURATION_OPTIONS.map(d => (
              <View key={d} className={`duration-chip ${residenceDuration === d ? 'active' : ''}`}
                onClick={() => setResidenceDuration(residenceDuration === d ? '' : d)}>
                {d}
              </View>
            ))}
          </View>
        </View>

        {/* 舌苔 / 左手手相 / 右手手相图片采集：望诊核心依据，要求拍照而非文字描述。
            手相分左右手两个独立槽位，且两者都可选——一张都不传也可以，模型靠其它信息推断。 */}
        <View className='card'>
          <View className='card-title'>体征图片采集</View>
          <Text className='card-note'>
            在自然光下拍摄：① 伸舌平展、不要卷曲；② 分别拍左手、右手手掌正面平放。
            照片越清晰，望诊越准。手相可完全不提供。
          </Text>
          <View className='image-slots'>
            <ImageSlot label='舌苔照片（伸舌平展）' image={tongueImage}
              onPick={() => pickImage(setTongueImage)}
              onClear={() => setTongueImage('')} />
          </View>
          <Text className='card-subnote'>手相：请分别上传左手、右手（均可不提供）</Text>
          <View className='image-slots'>
            <ImageSlot label='左手手相' image={palmLeftImage}
              onPick={() => pickImage(setPalmLeftImage)}
              onClear={() => setPalmLeftImage('')} />
            <ImageSlot label='右手手相' image={palmRightImage}
              onPick={() => pickImage(setPalmRightImage)}
              onClear={() => setPalmRightImage('')} />
          </View>
        </View>

        <View className='submit-wrap'>
          <View className={`btn-primary ${canStart ? '' : 'disabled'}`} onClick={startDiagnosis}>
            {busy ? '问诊中…' : '开始问诊'}
          </View>
          <Text className='disclaimer'>本服务由 AI 提供健康参考，不构成医疗诊断</Text>
        </View>
      </View>
    )
  }

  const steps = result.steps
  const current = steps[Math.min(activeIdx, steps.length - 1)]

  // 未定证时「还差哪些表现就能定证」（I3）。
  //
  // H3 产出的 `near` 里带着每个接近候选缺哪条主症，是规则确定性算出来的，
  // 不是模型编的。只摆一句「未匹配到明确证候」，用户根本不知道下一步该说什么；
  // 而后端既然已经算好，前端不展示等于白算（T7.10 的教训：
  // 前端不认的后端能力等于没做）。
  const hints = nearHints(result.structured?.differentiation)

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

      {/* 结论可信度提示（H4/H5）：未定证 / 置信度不足 / 强制放行。
          这些情形过去在响应里毫无痕迹，用户拿到的是一份看起来正常的报告，
          却不知道它建立在证据不足之上。与免责声明同级：必须看见，不能折叠。 */}
      {result.low_confidence && result.confidence_note && (
        <View className='confidence-banner'>
          <View className='confidence-note'><Markdown text={result.confidence_note} /></View>
          {hints.length > 0 && (
            <View className='near-block'>
              <Text className='near-tip'>补充下面这些表现，可能就能定证：</Text>
              {hints.map(n => (
                <View key={n.slug} className='near-item'>
                  <Text className='near-name'>{n.name}</Text>
                  <Text className='near-missing'>还缺：{n.missing.join('、')}</Text>
                </View>
              ))}
            </View>
          )}
        </View>
      )}

      <ScrollView className='step-body' scrollY>
        <Text className='step-title'>
          {CAPABILITY_ZH[current?.capability as HarnessCapability] || current?.capability || ''}
        </Text>
        {current?.text
          ? <Markdown className='step-text' text={current.text} />
          : <Text className='step-text'>（无输出）</Text>}
      </ScrollView>

      {/* 信息不足：后端已在辨证后停下（此时没有治疗建议），
          把待补条目直接摆出来，否则用户只看到一份「缺了后半截」的报告，
          根本不知道还该说什么。这些条目是规则确定性产出的，不是模型编的。 */}
      {result.status === 'awaiting_input' && (
        <View className='pending-panel'>
          <Text className='pending-tip'>
            信息还不足以下结论（已采集 {Math.round((result.loop?.coverage ?? 0) * 100)}%），
            点下面任一项补上，会重新辨证：
          </Text>
          {(result.loop?.pending_questions ?? []).slice(0, 4).map(q => (
            <View key={q.slug} className='pending-item'
              onClick={() => setInput(q.text)}>
              <Text className='pending-text'>{q.text}</Text>
            </View>
          ))}
        </View>
      )}

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
