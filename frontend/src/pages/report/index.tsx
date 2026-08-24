import { useEffect, useState } from 'react'
import Taro, { useRouter } from '@tarojs/taro'
import { View, Text, Textarea } from '@tarojs/components'
import { checkCare, getFollowups, getRevisitCompare, getState, postFollowupFeedback, postLab, postRevisit } from '../../services/api'
import { confidencePercent, categoryClass } from '../../utils/format'
import type { CareTodo, ConsultState, FollowUp, LabResult, RevisitCompare } from '../../types'
import './index.scss'

const KIND_LABEL: Record<CareTodo['kind'], string> = {
  decoct: '煎服提醒',
  checkin: '每日打卡',
  appointment: '预约/外治',
}

const FEATURE_LABEL: Record<string, string> = {
  'tongue.body': '舌色',
  'tongue.coat': '舌苔',
  'face.color': '面色',
}

const SRC_ALIAS: Record<string, string> = {
  望: '望', 闻: '闻', 问: '问', 切: '切', 脉: '切', 随访: '随',
}

function srcKey(src: string): string {
  return SRC_ALIAS[src] || '问'
}

interface ChainEv { text: string; src: string }
interface Chain { name: string; supporting: ChainEv[]; contradicting: ChainEv[] }

// 解析 reasoning：「证候名】支持证据：a（问）、b（望）；矛盾证据：c（切）」
function parseChains(reasoning: string): Chain[] {
  const out: Chain[] = []
  const blocks = reasoning.split(/【/).slice(1)
  for (const blk of blocks) {
    const m = blk.match(/^([^】]+)】(.*)$/s)
    if (!m) continue
    const name = m[1].trim()
    const body = m[2]
    const supRaw = (body.match(/支持证据：(.*?)(?:；矛盾证据：|$)/s)?.[1] || '').trim()
    const conRaw = (body.match(/矛盾证据：(.*)$/s)?.[1] || '').trim()
    const parse = (s: string): ChainEv[] =>
      s.split('、').map(x => x.trim()).filter(Boolean).map(x => {
        const mm = x.match(/^(.*)（(.+?)）$/)
        return mm ? { text: mm[1], src: mm[2] } : { text: x, src: '问' }
      })
    out.push({ name, supporting: parse(supRaw), contradicting: parse(conRaw) })
  }
  return out
}

export default function ReportPage() {
  const router = useRouter()
  const cid = router.params.id || ''
  const [state, setState] = useState<ConsultState | null>(null)
  const [care, setCare] = useState<CareTodo[]>([])
  const [followups, setFollowups] = useState<FollowUp[]>([])
  const [revisit, setRevisit] = useState<RevisitCompare | null>(null)
  const [lab, setLab] = useState<LabResult | null>(null)
  const [labOpen, setLabOpen] = useState(false)
  const [labText, setLabText] = useState('')
  const [fbId, setFbId] = useState<string | null>(null)
  const [fbText, setFbText] = useState('')
  const [expanded, setExpanded] = useState(false)

  useEffect(() => {
    if (!cid) return
    getState(cid)
      .then(s => { setState(s); setCare(s.care_todos || []) })
      .catch((e: any) => Taro.showToast({ title: e?.message || '加载失败', icon: 'none' }))
    getFollowups(cid).then(setFollowups).catch(() => {})
    getRevisitCompare(cid).then(setRevisit).catch(() => {})
  }, [cid])

  const uploadRevisit = async (kind: string) => {
    try {
      const res = await Taro.chooseImage({ count: 1 })
      const path = res.tempFilePaths[0]
      // 实际部署应先上传到后端 /upload 换取 URL；此处直接以本地路径触发望诊（演示）
      await postRevisit(cid, path, kind)
      const cmp = await getRevisitCompare(cid)
      setRevisit(cmp)
      Taro.showToast({ title: '复诊已记录', icon: 'success' })
    } catch (e: any) {
      if (e?.errMsg && e.errMsg.includes('cancel')) return
      Taro.showToast({ title: e?.message || '上传失败', icon: 'none' })
    }
  }

  const onCheck = async (id: string) => {
    setCare(prev => prev.map(t => t.id === id ? { ...t, done: true } : t))
    try { await checkCare(cid, id) } catch {}
  }

  const submitFeedback = async () => {
    if (!fbId || !fbText.trim()) return
    try {
      const res = await postFollowupFeedback(cid, fbId, fbText.trim())
      const fu = res.followup
      setFollowups(prev => prev.map(f => f.id === fu.id ? fu : f))
      Taro.showToast({ title: '已记录回访', icon: 'success' })
      setFbId(null)
      setFbText('')
    } catch (e: any) {
      Taro.showToast({ title: e?.message || '提交失败', icon: 'none' })
    }
  }

  const submitLab = async () => {
    if (!labText.trim()) return
    try {
      const res = await postLab(cid, labText.trim())
      setLab(res)
      setLabOpen(false)
      setLabText('')
      Taro.showToast({ title: '已融合解读', icon: 'success' })
    } catch (e: any) {
      Taro.showToast({ title: e?.message || '提交失败', icon: 'none' })
    }
  }

  if (!state?.report) return <View className='report-page' />
  const r = state.report
  const referred = state.status === 'referred'

  return (
    <View className='report-page'>
      {referred && (
        <View className='card danger-card'>
          <View className='card-title danger'>⚠ 需要立即就医</View>
          <Text>{r.red_flag}</Text>
          <Text className='danger-advice'>{r.advice['紧急建议']}</Text>
        </View>
      )}

      {!referred && (
        <View className='card'>
          <View className='card-title'>辨证结论</View>
          {r.syndromes.length === 0 && <Text>暂无法给出明确辨证，建议线下面诊。</Text>}
          {r.syndromes.map(s => (
            <View key={s.name} className='syndrome-row'>
              <Text className='syndrome-name'>{s.name}</Text>
              <Text className='syndrome-conf'>置信度 {confidencePercent(s.confidence)}</Text>
            </View>
          ))}
        </View>
      )}

      <View className='card'>
        <View className='card-title' onClick={() => setExpanded(!expanded)}>
          辨证依据（溯源） {expanded ? '▲' : '▼'}
        </View>
        {expanded ? (
          <View className='chains'>
            {parseChains(r.reasoning).map((ch, i) => (
              <View key={i} className='chain-block'>
                <Text className='chain-name'>{ch.name}</Text>
                <View className='chain-group'>
                  <Text className='chain-label sup'>支持</Text>
                  {ch.supporting.map((ev, j) => (
                    <Text key={j} className={`ev-tag src-${srcKey(ev.src)}`}>
                      {ev.text}<Text className='ev-src'>{ev.src}</Text>
                    </Text>
                  ))}
                </View>
                {ch.contradicting.length > 0 && (
                  <View className='chain-group'>
                    <Text className='chain-label con'>矛盾</Text>
                    {ch.contradicting.map((ev, j) => (
                      <Text key={j} className={`ev-tag src-${srcKey(ev.src)} con`}>
                        {ev.text}<Text className='ev-src'>{ev.src}</Text>
                      </Text>
                    ))}
                  </View>
                )}
              </View>
            ))}
          </View>
        ) : (
          <Text className='reasoning clamp'>{r.reasoning}</Text>
        )}
      </View>

      {!referred && r.treatments && r.treatments.length > 0 && (
        <View className='card'>
          <View className='card-title'>诊疗方案
            <Text className='sub-title'>（以更快、更彻底痊愈为目标）</Text>
          </View>
          {r.treatments.map(t => (
            <View key={t.id} className='plan-row'>
              <View className='plan-head'>
                <Text className={`plan-cat ${categoryClass(t.category)}`}>{t.category}</Text>
                <Text className='plan-title'>{t.title}</Text>
              </View>
              {t.detail && <Text className='plan-detail'>{t.detail}</Text>}
              {t.rationale && <Text className='plan-reason'>依据：{t.rationale}</Text>}
              {t.note && <Text className='plan-note'>注意：{t.note}</Text>}
              {(t.warnings || []).length > 0 && (
                <View className='plan-warn'>
                  {(t.warnings as string[]).map((w, i) => (
                    <Text key={i} className='warn-item'>⚠ {w}</Text>
                  ))}
                </View>
              )}
            </View>
          ))}
        </View>
      )}

      {!referred && r.evolution && (
        <View className='card'>
          <View className='card-title'>证候传变提示</View>
          <Text className='evolution-text'>{r.evolution}</Text>
        </View>
      )}

      {!referred && Object.keys(r.advice).length > 0 && (
        <View className='card'>
          <View className='card-title'>调理建议</View>
          {Object.entries(r.advice).map(([k, v]) => (
            <View key={k} className='advice-row'>
              <Text className='advice-key'>{k}</Text>
              <Text className='advice-val'>{v}</Text>
            </View>
          ))}
        </View>
      )}

      {r.sources && r.sources.length > 0 && (
        <View className='card'>
          <View className='card-title'>参考来源</View>
          <View className='src-list'>
            {r.sources.map(s => <Text key={s} className='src-chip'>《{s}》</Text>)}
          </View>
        </View>
      )}

      <View className='card disclaimer-card'>
        <Text className='disclaimer-text'>{r.disclaimer}</Text>
      </View>

      {!referred && care.length > 0 && (
        <View className='card'>
          <View className='card-title'>今日待办
            <Text className='sub-title'>（方案已拆成可执行打卡）</Text>
          </View>
          {care.map(t => (
            <View key={t.id} className={`care-row ${t.done ? 'done' : ''}`}>
              <View className='care-main'>
                <Text className='care-title'>{t.title}</Text>
                {t.detail && <Text className='care-detail'>{t.detail}</Text>}
                <Text className='care-meta'>
                  {KIND_LABEL[t.kind]}
                  {t.times.length > 0 ? ` · ${t.times.join(' / ')}` : ''}
                </Text>
              </View>
              <View className={`care-check ${t.done ? 'checked' : ''}`}
                onClick={() => !t.done && onCheck(t.id)}>
                {t.done ? '已打卡' : '完成'}
              </View>
            </View>
          ))}
        </View>
      )}

      {!referred && followups.length > 0 && (
        <View className='card'>
          <View className='card-title'>随访计划
            <Text className='sub-title'>（按恢复节奏回访，闭环调理）</Text>
          </View>
          {followups.map(t => (
            <View key={t.id} className={`fu-row ${t.done ? 'done' : ''}`}>
              <View className='fu-main'>
                <Text className='fu-title'>第 {t.due_in_days} 天回访</Text>
                <Text className='fu-focus'>关注：{t.focus}</Text>
                {t.done && t.feedback && (
                  <Text className='fu-feedback'>反馈：{t.feedback}</Text>
                )}
              </View>
              {!t.done && (
                <View className='fu-btn' onClick={() => setFbId(t.id)}>去反馈</View>
              )}
              {t.done && <Text className='fu-done-tag'>已回访</Text>}
            </View>
          ))}
        </View>
      )}

      {fbId && (
        <View className='fb-mask' onClick={() => setFbId(null)}>
          <View className='fb-sheet' onClick={e => e.stopPropagation()}>
            <View className='fb-title'>回访反馈（第 {followups.find(f => f.id === fbId)?.due_in_days} 天）</View>
            <Textarea className='fb-input' placeholder='描述当前症状变化、服药反应等...'
              value={fbText} onInput={e => setFbText(e.detail.value)} />
            <View className='fb-actions'>
              <View className='fb-cancel' onClick={() => setFbId(null)}>取消</View>
              <View className={`fb-submit ${fbText.trim() ? '' : 'disabled'}`}
                onClick={submitFeedback}>提交</View>
            </View>
          </View>
        </View>
      )}

      {!referred && (
        <View className='card'>
          <View className='card-title'>舌象复诊对比
            <Text className='sub-title'>（上传复诊舌象，量化恢复趋势）</Text>
          </View>
          {revisit && revisit.has_baseline && revisit.changes.length > 0 && (
            <View className='rv-changes'>
              {revisit.changes.map((ch, i) => (
                <View key={i} className='rv-row'>
                  <Text className='rv-feat'>{FEATURE_LABEL[ch.key] || ch.key}</Text>
                  <Text className='rv-before'>{ch.before}</Text>
                  <Text className='rv-arrow'>
                    {ch.improved === 'better' ? '→ 好转' : ch.improved === 'worse' ? '→ 加重' : '→ 变化'}
                  </Text>
                  <Text className='rv-after'>{ch.after}</Text>
                </View>
              ))}
            </View>
          )}
          {revisit && revisit.has_baseline && revisit.changes.length === 0 && (
            <Text className='rv-empty'>暂无变化，望诊特征与首诊一致。</Text>
          )}
          {(!revisit || !revisit.has_baseline) && (
            <Text className='rv-empty'>首诊未采集到望诊特征，无法对比。</Text>
          )}
          <View className='rv-actions'>
            <View className='rv-btn' onClick={() => uploadRevisit('tongue')}>上传复诊舌象</View>
            <View className='rv-btn alt' onClick={() => uploadRevisit('face')}>上传面色</View>
          </View>
        </View>
      )}

      {!referred && (
        <View className='card'>
          <View className='card-title'>检验报告解读
            <Text className='sub-title'>（中西医结合，指标+证候互参）</Text>
          </View>
          {lab ? (
            <View className='lab-result'>
              {lab.indicators.length > 0 && (
                <View className='lab-indicators'>
                  {lab.indicators.map((ind, i) => (
                    <Text key={i} className={`lab-ind ${ind.abnormal ? 'abn' : ''}`}>
                      {ind.name}：{ind.value}{ind.abnormal ? ' ⚠' : ''}
                    </Text>
                  ))}
                </View>
              )}
              {lab.tcm_note && <Text className='lab-note'>{lab.tcm_note}</Text>}
              {lab.evidence_keys.length > 0 && (
                <Text className='lab-ev'>已并入辨证证据：{lab.evidence_keys.join('、')}</Text>
              )}
            </View>
          ) : (
            <Text className='rv-empty'>上传血常规/影像等报告文本，AI 融合中西医视角解读。</Text>
          )}
          <View className='rv-actions'>
            <View className='rv-btn' onClick={() => setLabOpen(true)}>
              {lab ? '重新解读' : '上传检验报告'}
            </View>
          </View>
        </View>
      )}

      {labOpen && (
        <View className='fb-mask' onClick={() => setLabOpen(false)}>
          <View className='fb-sheet' onClick={e => e.stopPropagation()}>
            <View className='fb-title'>检验报告文本</View>
            <Textarea className='fb-input' placeholder='粘贴报告中的异常指标与数值...'
              value={labText} onInput={e => setLabText(e.detail.value)} />
            <View className='fb-actions'>
              <View className='fb-cancel' onClick={() => setLabOpen(false)}>取消</View>
              <View className={`fb-submit ${labText.trim() ? '' : 'disabled'}`}
                onClick={submitLab}>解读</View>
            </View>
          </View>
        </View>
      )}

      <View className='submit-wrap'>
        <View className='btn-primary'
          onClick={() => Taro.reLaunch({ url: '/pages/index/index' })}>
          再次问诊
        </View>
      </View>
    </View>
  )
}
