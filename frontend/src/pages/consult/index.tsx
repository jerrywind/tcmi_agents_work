import { useEffect, useRef, useState } from 'react'
import Taro from '@tarojs/taro'
import { View, Text, Input, Textarea, ScrollView, Image } from '@tarojs/components'
import { CAPABILITY_ZH, chat, SUPPORTS_FULL_CHAT } from '../../services/harness'
import { streamChat } from '../../services/stream'
import type { StepState, StreamHandle } from '../../services/stream'
import {
  advanceRound, getMessages, getPayload, getProfile, getResult, pushMessage, resetMessages,
  rollbackSession, setResult, snapshotSession,
} from '../../services/session'
import { nearHints } from '../../utils/differentiation'
import { applyStreamEvent, emptyAcc } from '../../utils/streamAcc'
import type { StreamAcc } from '../../utils/streamAcc'
import { SyndromeCard } from '../../components/SyndromeCard'
import { useTypewriter } from '../../utils/useTypewriter'
import { buildOpeningMessages, buildResidenceLine, describeProfile, RESIDENCE_DURATION_OPTIONS } from '../../utils/profile'
import { Markdown } from '../../utils/markdown'
import { chooseImageAsDataURL } from '../../utils/image'
import { ErrorBoundary } from '../../components/ErrorBoundary'
import type { DiagnosisResult, HarnessCapability } from '../../types'
import './index.scss'

/**
 * 流式步骤卡片：用打字机把「整段突然出现」变成逐字写出。
 *
 * 交互要点：点一下立即补齐全文（`flush`）。动画是给等待用的，
 * 如果用户想读完却被迫等动画，那就是帮倒忙。
 */
function StreamCard({ title, text, stale }: { title: string; text: string; stale?: boolean }) {
  // 上一轮占位**不做打字机**：它本来就已经完整，再逐字播一遍既慢又像新内容
  const { shown, flush } = useTypewriter(text, { disabled: stale })
  return (
    <View className={`stream-card ${stale ? 'stream-card-stale' : ''}`} onClick={flush}>
      <Text className='stream-card-title'>{title}</Text>
      {/* 打字过程中按「块」交给 Markdown 渲染：逐字渲染半截 Markdown
          会让语法符号闪来闪去，观感比不渲染还差。 */}
      <Markdown className='stream-card-text' text={shown} />
    </View>
  )
}

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
/**
 * 问诊页：外层包错误边界。
 *
 * 本页链路最长（图片采集 → 两百秒以上的请求 → Markdown 渲染 → 多轮状态），
 * 任一处渲染抛错都不该变成白屏。
 */
export default function ConsultPage() {
  return (
    <ErrorBoundary title='问诊页出错了'>
      <ConsultInner />
    </ErrorBoundary>
  )
}

function ConsultInner() {
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
  // 选图是异步的：不加这个守卫，连点会同时唤起多个选择器
  const [picking, setPicking] = useState(false)

  /**
   * 选图：把结果写进对应 slot。
   *
   * 用户**主动取消**时静默（不打扰），真实失败才提示——
   * 两者此前都返回 null，点了没反应时用户分不清是取消了还是坏了。
   * `picking` 挡住重复点击：选图是异步的，连点会同时唤起多个选择器。
   */
  const pickImage = async (setter: (v: string) => void) => {
    if (picking) return
    setPicking(true)
    // 不用 try/finally：`chooseImageAsDataURL` 保证不抛（失败一律走返回值），
    // 而 try 块里 await 之后 TypeScript 不再对判别联合做收窄，
    // 包一层 try 反而会让下面访问 reason / message 编译不过。
    const r = await chooseImageAsDataURL()
    setPicking(false)

    if (r.status === 'ok') {
      setter(r.dataUrl)
    } else if (r.status === 'error') {
      // 主动取消（status === 'cancel'）不打扰，只有真的出问题才提示
      Taro.showToast({ title: r.message || '读取图片失败', icon: 'none' })
    }
  }

  /** 把已采集的图片整理成后端约定的 `images` 数组（无图则为空）。 */
  const collectedImages = () => {
    const imgs: { kind: string; data_url: string }[] = []
    if (tongueImage) imgs.push({ kind: 'tongue', data_url: tongueImage })
    if (palmLeftImage) imgs.push({ kind: 'palm_left', data_url: palmLeftImage })
    if (palmRightImage) imgs.push({ kind: 'palm_right', data_url: palmRightImage })
    return imgs
  }

  // ---------------- 流式问诊状态 ----------------
  //
  // 一次完整问诊 200–530 秒。非流式时前端只能挂一句 loading：用户既不知道
  // 进行到第几步，也不知道还要等多久。**流式把这段时间切成事件流**——
  // 骨架（`hello`）先到，之后每完成一步补一段内容，感知延迟从「几分钟」
  // 降到「一秒出骨架、之后持续有反馈」。
  const [streaming, setStreaming] = useState(false)
  const [elapsed, setElapsed] = useState(0)
  const [snap, setSnap] = useState<StreamAcc | null>(null)
  // 请求句柄：页面卸载/用户取消时要 abort，否则后端会继续把 10 步跑完
  const streamRef = useRef<StreamHandle | null>(null)
  /**
   * 「撤销本次提交」的回调，在发起请求前设好。
   *
   * 用户中途要改描述时，已经 push 进 session 的消息和已推进的轮次都得退回去，
   * 否则点一次「重填」就多发一遍主诉（首诊）或白吃一轮追问预算（追问）。
   * 两种情形的回滚方式不同，故在这里统一成一个回调。
   */
  const undoRef = useRef<(() => void) | null>(null)

  // 秒表：让用户看见「时间在动」，比一个转圈更能说明没卡死
  useEffect(() => {
    if (!streaming) return
    const t = setInterval(() => setElapsed(s => s + 1), 1000)
    return () => clearInterval(t)
  }, [streaming])

  // 离开页面就取消：用户都走了，后端没必要继续烧 LLM
  useEffect(() => () => streamRef.current?.cancel(), [])

  /**
   * 小程序端跑不了完整问诊：一次 `/chat` 要 200–530 秒，而平台请求上限 60 秒。
   *
   * 在发起前就拦住，别让用户干等 60 秒只等来一句超时。
   * 也不假装能跑——分步调 `/agents` 拼出来的流程会丢掉安全门拦截
   * （详见 `SUPPORTS_FULL_CHAT` 的注释），那比「不支持」危险得多。
   */
  const chatBlocked = () => {
    if (SUPPORTS_FULL_CHAT) return false
    Taro.showModal({
      title: '当前端暂不支持完整问诊',
      content: '完整问诊需要连续计算 3–9 分钟，而小程序平台限制单次请求 60 秒。请在手机浏览器中打开 H5 版使用。',
      showCancel: false,
    })
    return true
  }

  // 直达本页却没有档案：payload 无从构造，退回档案页
  useEffect(() => {
    if (!profile) Taro.redirectTo({ url: '/pages/index/index' })
  }, [profile])

  /**
   * 中断本次问诊并退回可编辑状态（供「描述有误？中断重填」使用）。
   *
   * 后端是「断连即停」的（见 `stream.rs` 的 `is_closed`），
   * 所以这里 cancel 之后不会再白白烧掉剩下几步的 LLM 调用。
   */
  const abortAndRefill = () => {
    streamRef.current?.cancel()
    streamRef.current = null
    undoRef.current?.()
    undoRef.current = null
    setStreaming(false)
    setBusy(false)
    setSnap(null)
  }

  /**
   * 跑一次流式问诊，结束时把累加出的结果落成 `DiagnosisResult`。
   *
   * 为什么要在这里组装成 `DiagnosisResult` 而不是另搞一套状态：
   * 报告页、存证、追问链路全都读 `session.getResult()`。流式只是**取数方式**
   * 变了，产物必须还是同一个形状，否则「问诊页能看、报告页空的」——
   * 这类「前端不认的后端能力等于没做」的问题，正是之前踩过的。
   */
  const runStream = (
    payload: Record<string, any>,
    onFail: (e: Error) => void,
    /** 上一轮各步正文（按 capability）：作为占位先显示，新结果到达后覆盖 */
    keep: Record<string, string> = {},
  ) => {
    const acc = emptyAcc()
    acc.keep = keep
    // `acc` 是唯一数据源，`snap` 只是给 React 渲染用的快照
    const publish = () => setSnap({ ...acc })
    setStreaming(true)
    setElapsed(0)
    publish()

    const finalize = () => {
      const idxs = Object.keys(acc.texts).map(Number).sort((a, b) => a - b)
      const r: DiagnosisResult = {
        steps: idxs.map(i => ({
          capability: acc.texts[i].capability,
          text: acc.texts[i].text,
        })) as DiagnosisResult['steps'],
        summary: acc.summary,
        failures: acc.failures as DiagnosisResult['failures'],
        partial: acc.failures.length > 0,
        blocked: !!acc.blocked,
        block_reason: acc.blocked
          ? `${acc.blocked.label}·${acc.blocked.severity}：${acc.blocked.advice}`
          : null,
        skipped: acc.skipped as DiagnosisResult['skipped'],
        structured: Object.keys(acc.structured).length ? acc.structured : null,
        report_id: acc.reportId,
        disclaimer: acc.disclaimer,
        status: acc.loop && !acc.loop.converged ? 'awaiting_input' : 'completed',
        loop: acc.loop,
        low_confidence: acc.confidence?.low,
        confidence_note: acc.confidence?.note,
      }
      setResult(r)
      setLocalResult(r)
      setActiveIdx(0)
      setStreaming(false)
      setBusy(false)
    }

    streamRef.current = streamChat({
      messages: getMessages(),
      payload,
      onEvent: e => {
        applyStreamEvent(acc, e)
        publish()
      },
      onError: err => {
        setStreaming(false)
        setBusy(false)
        // 一步都没收到就失败：多半是流式链路本身不通（代理缓冲、旧版后端…）。
        // 此时回退到非流式 `/chat`，用户至少还能拿到结果，
        // 而不是被一条「网络异常」挡在门外。
        if (!acc.gotStep) onFail(err)
        else Taro.showToast({ title: err.message || '问诊中断', icon: 'none' })
      },
      onDone: finalize,
    })
  }

  /** 非流式兜底：流式链路不通时的退路（详见 `runStream` 的 `onError`） */
  const runPlain = async (payload: Record<string, any>, onFail: (e: Error) => void) => {
    try {
      Taro.showLoading({ title: '问诊中，请稍候…' })
      const r = await chat(getMessages(), payload)
      setResult(r)
      setLocalResult(r)
      setActiveIdx(0)
      Taro.hideLoading()
    } catch (e: any) {
      Taro.hideLoading()
      onFail(e instanceof Error ? e : new Error(e?.message || '问诊失败'))
    }
  }

  /** 首诊：主诉 + 档案里的既往病史一起发出去。首诊仍是第 1 轮，不推进轮次。 */
  const startDiagnosis = () => {
    const text = complaint.trim()
    if (text.length < 5 || busy || !profile) return
    // 端能力校验排在输入校验之后：主诉没填时先提示填主诉，别弹「端不支持」
    if (chatBlocked()) return
    setBusy(true)
    setComplaint('')
    // 既往病史独立成条、排在主诉之前（理由见 buildOpeningMessages 注释）
    const opening = buildOpeningMessages(text, profile.history || '')
    // 当前居住地作为上下文，插到主诉之前（与既往病史同级）；缺则整条不注入
    const resLine = buildResidenceLine(residence, residenceDuration)
    if (resLine) opening.splice(opening.length - 1, 0, { role: 'user', content: resLine })
    opening.forEach(pushMessage)

    // 首诊失败要把已经推进去的主诉撤回来，否则重试一次就多发一遍
    const onFail = (e: Error) => {
      resetMessages()
      setComplaint(text)
      undoRef.current = null
      Taro.showToast({
        title: e.message || '问诊失败，请确认后端已启动且 LLM 可用',
        icon: 'none',
      })
    }
    // 中断重填时按同一套方式回滚（首诊：撤回主诉并还回输入框）
    undoRef.current = () => {
      resetMessages()
      setComplaint(text)
    }
    runStream({ ...getPayload(), images: collectedImages() }, err => {
      Taro.showToast({ title: `${err.message || '流式中断'}，改用普通模式`, icon: 'none' })
      void runPlain({ ...getPayload(), images: collectedImages() }, onFail)
    })
  }

  const ask = () => {
    const text = input.trim()
    if (!text || busy) return
    if (chatBlocked()) return
    setBusy(true)
    setInput('')
    // 与首诊同理：消息和轮次都在请求之前就落地了，失败要整体撤回。
    // 只回滚消息不回滚轮次的话，每失败一次就白吃一轮追问预算。
    const snapshot = snapshotSession()
    pushMessage({ role: 'user', content: text })
    // 在已有结论之上补充 → 轮次 +1，后端的「达到上限强制放行」才可能触发
    advanceRound()

    const onFail = (e: Error) => {
      rollbackSession(snapshot)
      // 把用户辛苦打的字还回输入框，别让他重打一遍
      setInput(text)
      undoRef.current = null
      Taro.showToast({ title: e.message || '追问失败', icon: 'none' })
    }
    // 追问中断：消息与轮次一起退回，否则白吃一轮追问预算
    undoRef.current = () => {
      rollbackSession(snapshot)
      setInput(text)
    }
    // 上一轮各步正文作为占位：后端会把 10 步全部重跑，
    // 没有它屏幕会先变空白再慢慢长出来（详见 StreamAcc.keep 的注释）
    const keep: Record<string, string> = {}
    for (const s of getResult()?.steps || []) {
      if (s.text) keep[s.capability] = s.text
    }
    runStream({ ...getPayload(), images: collectedImages() }, err => {
      Taro.showToast({ title: `${err.message || '流式中断'}，改用普通模式`, icon: 'none' })
      void runPlain({ ...getPayload(), images: collectedImages() }, onFail)
    }, keep)
  }

  // ---------------- 流式进行中的视图 ----------------
  //
  // 骨架（`hello` 下发的步骤计划）在第一帧就位，之后每完成一步补一块内容。
  // 用户从第一秒就能看到「要跑 7 步、现在第 3 步、已经用了 1 分 20 秒」——
  // 总耗时没变，但「不知道还要等多久」的焦虑消失了。
  if (streaming && snap) {
    const states = snap.states
    const doneCount = Object.keys(states).filter(k => states[Number(k)] === 'done').length
    const total = snap.plan.length || Math.max(1, doneCount)
    const running = snap.plan.find(s => states[s.index] === 'running')
    const mm = String(Math.floor(elapsed / 60)).padStart(2, '0')
    const ss = String(elapsed % 60).padStart(2, '0')
    const clsOf = (s?: StepState) => (s === 'done' ? 'phase done'
      : s === 'running' ? 'phase running'
        : s === 'failed' ? 'phase failed'
          : s === 'skipped' ? 'phase skipped' : 'phase')

    return (
      <View className='consult-page'>
        <View className='progress-bar'>
          <ScrollView className='phases' scrollX>
            {snap.plan.map(s => (
              <View key={s.index} className={clsOf(states[s.index])}>
                <Text>{s.zh}</Text>
              </View>
            ))}
          </ScrollView>
        </View>

        <View className='stream-meta'>
          <Text className='stream-meta-text'>
            已完成 {doneCount}/{total} 步 · 用时 {mm}:{ss}
            {running ? ` · 正在${running.zh}` : ''}
          </Text>
        </View>

        {/* 「我读到的表现」：首帧就到（规则抽取，不等 LLM）。
            读错了现在纠正只花 1 秒，等几分钟看到结论才发现就白跑一轮。 */}
        {snap.understood.length > 0 && (
          <View className='understood-bar'>
            <Text className='understood-label'>我读到的表现</Text>
            <View className='understood-tags'>
              {snap.understood.map(t => (
                <Text key={t} className='understood-tag'>{t}</Text>
              ))}
            </View>
            {/* 中断重填**只在首诊给**：追问阶段主诉已锁定，改不了了。
                在追问里摆这个入口，点了会退回到上一轮结论——
                用户以为能改主诉，实际只丢掉这一轮，是误导。
                后端有「断连即停」，取消后不会再烧剩下几步 LLM。 */}
            {!result && (
              <Text className='understood-fix' onClick={abortAndRefill}>描述有误？中断重填</Text>
            )}
          </View>
        )}

        {/* 安全门拦截：命中就置顶，并明确「已停止后续治疗建议」。
            它是整次问诊里最该被看到的信息，不能排在逐步内容之后。 */}
        {snap.blocked && (
          <View className='blocked-banner'>
            <Text className='blocked-title'>安全门拦截：{snap.blocked.label}</Text>
            <Text className='blocked-text'>{snap.blocked.advice}</Text>
            <Text className='blocked-text'>已停止后续治疗建议，请及时就医。</Text>
          </View>
        )}

        {/* 结论可信度：与拦截同级，必须看见、不能折叠 */}
        {snap.confidence?.low && snap.confidence.note && (
          <View className='confidence-banner'>
            <View className='confidence-note'><Markdown text={snap.confidence.note} /></View>
          </View>
        )}

        {/* 辨证结构**一算出来就插入**，不必等流程跑完。
            此前它只在最终报告页出现，问诊中要等 200 秒才看得见——
            而这是规则确定性算出来的结论，捂到最后展示等于白算。 */}
        {snap.structured?.differentiation?.primary && (
          <View className='stream-diff'>
            <Text className='stream-diff-title'>辨证结构</Text>
            <SyndromeCard kind='主证' s={snap.structured.differentiation.primary} compact />
            {(snap.structured.differentiation.concurrent || []).map((c: any) => (
              <SyndromeCard key={c.slug} kind='兼证' s={c} compact />
            ))}
          </View>
        )}

        <ScrollView className='step-body' scrollY>
          {/* `hello` 丢了（旧版后端 / 首帧被缓冲）时退化成按已完成步骤渲染，
              不能因为拿不到计划就把已经算出来的内容也藏起来。 */}
          {/* 已完成（texts）与生成中（partial）的步骤一并渲染：
              `partial` 让模型还在写的时候就已经有字在屏幕上长出来。 */}
          {(snap.plan.length
            ? snap.plan.filter(
              s => snap.texts[s.index] || snap.partial[s.index] || snap.keep[s.capability],
            )
            : Object.keys({ ...snap.texts, ...snap.partial }).map(Number).map(i => ({
              index: i,
              zh: snap.texts[i]?.zh || '',
              capability: snap.texts[i]?.capability || '',
              phase: 'diagnosis' as const,
            }))
          ).map(s => {
            const fresh = snap.texts[s.index]?.text || snap.partial[s.index]
            // 重试中要在标题上写出来：这一步可能有 6 分钟没有任何新内容，
            // 不说清楚用户只能以为卡死了（单次超时 120s × 最多 3 次尝试）。
            const rt = snap.retries[s.index]
            const title = rt
              ? `${s.zh}（重试 ${rt.attempt}/${rt.maxRetries}）`
              : (fresh ? s.zh : `${s.zh}（上一轮）`)
            // 有本轮内容就用本轮的；否则先摆上一轮的，并明确标注
            return (
              <StreamCard key={s.index}
                title={title}
                stale={!fresh}
                text={fresh || snap.keep[s.capability] || ''} />
            )
          })}
          {doneCount === 0 && Object.keys(snap.keep).length === 0 && (
            <Text className='stream-placeholder'>
              正在采集四诊信息，完成后会逐步显示…
            </Text>
          )}
        </ScrollView>

        {/* 收敛判定已出但信息不足：先告诉用户还差什么，
            等流程结束就能直接点选补充，不必等看完报告才明白。 */}
        {snap.loop && !snap.loop.converged && (snap.loop.pending_questions || []).length > 0 && (
          <View className='pending-panel'>
            <Text className='pending-tip'>
              信息还不足以下结论（已采集 {Math.round((snap.loop.coverage ?? 0) * 100)}%），
              稍后可补充：{(snap.loop.pending_questions || []).slice(0, 3)
                .map((q: any) => q.text).join('、')}
            </Text>
          </View>
        )}
      </View>
    )
  }

  // 首诊前：只让用户描述这一次的病情。
  //
  // 这里刻意用 `.form-flow` 走普通文档流，而不是「进度条 + 步骤区 + 底部固定栏」
  // 的固定视口布局：首诊是一张长表单（病情自述 + 居住地 + 三张体征图 + 提交），
  // 套进 100vh 的 flex 容器会让它失去滚动能力，底部提交按钮够不到。
  if (!result) {
    const canStart = complaint.trim().length >= 5 && !busy && !!profile
    return (
      <View className='consult-page form-flow'>
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
      {/* 辨证结构在**结论态**同样要展示。
          只放在流式视图里是不够的：辨证往往是最后一步，卡片刚出现流程就结束了，
          一闪而过等于没有。结论态才是用户真正停下来读的地方。 */}
      {result.structured?.differentiation?.primary && (
        <View className='stream-diff'>
          <Text className='stream-diff-title'>辨证结构</Text>
          <SyndromeCard kind='主证' s={result.structured.differentiation.primary} compact />
          {(result.structured.differentiation.concurrent || []).map((c: any) => (
            <SyndromeCard key={c.slug} kind='兼证' s={c} compact />
          ))}
        </View>
      )}

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
