import type { HarnessMessage } from '../services/harness'
import type { Member, PatientProfile } from '../types'

// 出生日期选择相关的纯逻辑（补零、月天数、默认定位日期等）统一放在 birthdate.ts，
// 这里只做转发，避免两份实现漂移。
export { defaultBirthDate } from './birthdate'

import { daysInMonth } from './birthdate'

/**
 * 档案表单与体质档案之间的转换。
 *
 * 「创建档案」只让用户填五项：姓名（选填）、出生日期、性别、常住地、既往病史。
 * 这里是这五项唯一的处理入口，全部做成纯函数——
 * 周岁算错、性别传错、既往病史拼错位置都属于**静默失效**
 * （不报错，只是结论偏了），必须有单测钉住。
 */

/**
 * 性别选项。
 *
 * 不用 Picker 而用点选标签：Picker 打开后一格不滑直接点「确定」也会触发
 * onChange 写入定位值（出生日期那条已经踩过一次），把人静默设成默认性别
 * 没有任何提示。点选标签没有"未操作也生效"的默认值，不选就是不选。
 */
export const GENDER_OPTIONS = ['男', '女', '不愿透露']

/** 档案表单（与创建/编辑档案的表单项一一对应）。 */
export interface ProfileForm {
  name: string
  /** `YYYY-MM-DD` */
  birthDate: string
  /** `GENDER_OPTIONS` 之一，未选时为空串 */
  gender: string
  region: string
  history: string
}

export const EMPTY_PROFILE_FORM: ProfileForm = {
  name: '',
  birthDate: '',
  gender: '',
  region: '',
  history: '',
}

/** 表单里的性别 -> `payload.gender`。未选或「不愿透露」都落到后端的 Unknown。 */
export function toPayloadGender(gender: string): string {
  const g = (gender || '').trim()
  return g === '男' || g === '女' ? g : '未知'
}

/** `payload.gender` -> 表单里的性别。Unknown 视为"未选"，让用户重新选。 */
export function fromPayloadGender(gender: string | undefined): string {
  return gender === '男' || gender === '女' ? gender : ''
}

const DATE_RE = /^(\d{4})-(\d{2})-(\d{2})$/

/** 今天（`YYYY-MM-DD`）：出生日期选择器只能选到今天。 */
export function todayISO(today: Date = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${today.getFullYear()}-${pad(today.getMonth() + 1)}-${pad(today.getDate())}`
}

/** 出生日期是否是一个真实存在、且不晚于今天的日期。 */
export function isValidBirthDate(birthDate: string, today: Date = new Date()): boolean {
  const m = DATE_RE.exec((birthDate || '').trim())
  if (!m) return false
  const y = Number(m[1])
  const mo = Number(m[2])
  const d = Number(m[3])
  if (mo < 1 || mo > 12) return false
  if (d < 1 || d > daysInMonth(y, mo)) return false

  const born = new Date(y, mo - 1, d).getTime()
  const now = new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime()
  if (born > now) return false
  // 130 岁基本可以判定为误填（手滑选到 19xx 之外或未来年份）
  return today.getFullYear() - y <= 130
}

/**
 * 由出生日期算周岁。
 *
 * 返回 `undefined` 表示算不出来（空值 / 格式不对 / 未来日期），
 * 调用方应当**保持字段缺失**而不是填 0——`age=0` 会让下游把患者当成婴儿。
 */
export function calcAge(birthDate: string, today: Date = new Date()): number | undefined {
  if (!isValidBirthDate(birthDate, today)) return undefined
  const m = DATE_RE.exec(birthDate.trim())!
  const y = Number(m[1])
  const mo = Number(m[2])
  const d = Number(m[3])

  let age = today.getFullYear() - y
  const beforeBirthday =
    today.getMonth() + 1 < mo || (today.getMonth() + 1 === mo && today.getDate() < d)
  if (beforeBirthday) age -= 1
  return age
}

/**
 * 表单校验：只卡**影响结论**的三项。
 *
 * 姓名与既往病史是选填：前者只是称呼，后者没有就不注入。
 * 性别缺失时后端按「不排除」处理（男女专属条目都问），既浪费轮次
 * 也可能问出与患者不符的内容；出生日期与常住地缺失则让年龄、
 * 地域倾向整条线索断掉——这三项必须拦。
 */
export function validateProfileForm(form: ProfileForm, today: Date = new Date()): string {
  if (!form.birthDate.trim()) return '请选择出生日期'
  if (!isValidBirthDate(form.birthDate, today)) return '出生日期不合法'
  if (!form.gender.trim()) return '请选择性别'
  if (!form.region.trim()) return '请填写常住地'
  return ''
}

/** 表单 -> `payload` 用的体质档案。 */
export function buildProfile(form: ProfileForm, today: Date = new Date()): PatientProfile {
  const age = calcAge(form.birthDate, today)
  return {
    name: form.name.trim() || undefined,
    birth_date: form.birthDate.trim() || undefined,
    region: form.region.trim() || undefined,
    history: form.history.trim() || undefined,
    age,
    gender: toPayloadGender(form.gender),
  }
}

/** 体质档案 -> 表单（编辑已有档案时回填）。 */
export function toProfileForm(p: PatientProfile): ProfileForm {
  return {
    name: p.name || '',
    birthDate: p.birth_date || '',
    gender: fromPayloadGender(p.gender),
    region: p.region || '',
    history: p.history || '',
  }
}

/**
 * 首轮问诊的两条消息。
 *
 * 既往病史**必须排在本次主诉之前**，且是独立一条：
 * 望诊/闻诊/切诊都只取**最后一条 user 消息**做证据匹配，
 * 混进主诉正文会被当成当前症状（"既往高血压"变成"现在高血压"）；
 * 而安全门 `safety_corpus` 汇总**所有** user 消息，
 * 放在前面照样能被安全门读到——那正是过敏史、慢病最该去的地方。
 */
export function buildOpeningMessages(complaint: string, history: string): HarnessMessage[] {
  const out: HarnessMessage[] = []
  const h = (history || '').trim()
  if (h) out.push({ role: 'user', content: `【既往病史】${h}` })
  const c = (complaint || '').trim()
  if (c) out.push({ role: 'user', content: c })
  return out
}

/** 居住时长选项：近期所在地相关的辨证线索（水土不服、外邪入侵等）。 */
export const RESIDENCE_DURATION_OPTIONS = ['3天内', '一周内', '一个月内', '3个月内', '长期']

/**
 * 首轮问诊里「当前居住地」这一条 user 消息。
 *
 * 与档案页的「常住地」（长期居住地）不同：这里是**近期所在地 + 住了多久**，
 * 用来判断是否为新到异地（水土不服、时令外邪）。两者都没有则不注入。
 * 与既往病史同级、排在主诉之前，作为上下文；缺了它不影响主诉作为最后一条
 * user 消息被望闻切证据匹配。
 */
export function buildResidenceLine(place: string, duration: string): string | null {
  const p = (place || '').trim()
  const d = (duration || '').trim()
  if (!p && !d) return null
  const seg = p ? `当前所在地：${p}` : ''
  const seg2 = d ? `居住时长：${d}` : ''
  return `【当前居住地】${[seg, seg2].filter(Boolean).join('；')}`
}

/** 成员卡片上的一行摘要：`36岁 · 男 · 广州 · 既往：高血压`。 */
export function describeProfile(p: PatientProfile, today: Date = new Date()): string {
  const parts: string[] = []
  const age = p.age ?? calcAge(p.birth_date || '', today)
  if (age !== undefined) parts.push(`${age}岁`)
  else if (p.birth_date) parts.push(p.birth_date)
  if (p.gender === '男' || p.gender === '女') parts.push(p.gender)
  if (p.region) parts.push(p.region)
  if (p.history) parts.push(`既往：${p.history}`)
  return parts.join(' · ')
}

/**
 * 兼容已经存到本机的旧版成员档案。
 *
 * 旧结构带 `gender` / `height_cm` / `weight_kg` / `note`，且只有 `age`
 * 没有出生日期。本地存储不会因为代码改版而自动升级，
 * 不兼容就会让老用户的成员卡片显示成一串空白。
 * 旧档案里已经采集到的性别要**保住**——那是唯一有价值的遗留字段。
 */
export function normalizeMember(raw: any): Member | null {
  if (!raw || typeof raw !== 'object') return null
  const p = raw.patient && typeof raw.patient === 'object' ? raw.patient : {}
  const history = String(p.history ?? raw.note ?? '').trim()
  return {
    id: String(raw.id || ''),
    name: String(raw.name || '未命名'),
    relation: String(raw.relation || '其他'),
    patient: {
      name: p.name ? String(p.name) : undefined,
      birth_date: p.birth_date ? String(p.birth_date) : undefined,
      region: p.region ? String(p.region) : undefined,
      history: history || undefined,
      // 旧档案只存了年龄没有出生日期：保留它，卡片才不至于空白
      age: typeof p.age === 'number' ? p.age : undefined,
      gender: toPayloadGender(String(p.gender || '')),
    },
    note: undefined,
  }
}
