import { describe, it, expect } from 'vitest'
import {
  EMPTY_PROFILE_FORM, GENDER_OPTIONS, buildOpeningMessages, buildProfile, calcAge,
  defaultBirthDate, describeProfile, fromPayloadGender, isValidBirthDate, normalizeMember,
  toPayloadGender, toProfileForm, validateProfileForm,
} from './profile'

const TODAY = new Date(2026, 8, 4) // 2026-09-04

describe('calcAge 周岁计算', () => {
  it('生日已过，按整岁计', () => {
    expect(calcAge('1990-01-01', TODAY)).toBe(36)
  })

  it('生日未到，减一岁', () => {
    expect(calcAge('1990-12-31', TODAY)).toBe(35)
  })

  it('生日当天算满', () => {
    expect(calcAge('1990-09-04', TODAY)).toBe(36)
  })

  it('出生当天为 0 岁', () => {
    expect(calcAge('2026-09-04', TODAY)).toBe(0)
  })

  it('闰日 2 月 29 日合法', () => {
    expect(calcAge('2000-02-29', TODAY)).toBe(26)
  })

  it('空值 / 格式错误 / 月日越界一律返回 undefined，不填 0', () => {
    // age=0 会让下游把患者当成婴儿，宁可字段缺失
    expect(calcAge('', TODAY)).toBeUndefined()
    expect(calcAge('1990/01/01', TODAY)).toBeUndefined()
    expect(calcAge('1990-13-01', TODAY)).toBeUndefined()
    expect(calcAge('2026-02-31', TODAY)).toBeUndefined()
  })

  it('未来日期与超龄日期判为无效', () => {
    expect(calcAge('2027-01-01', TODAY)).toBeUndefined()
    expect(calcAge('1800-01-01', TODAY)).toBeUndefined()
  })
})

describe('defaultBirthDate 选择器定位日期', () => {
  it('定位到今天往前 30 年，而不是今天', () => {
    // 日期选择器点开再点「确定」就会触发 onChange，哪怕一格没滑。
    // 定位到今天会静默写入 age=0，把成年人当成新生儿。
    expect(defaultBirthDate(TODAY)).toBe('1996-09-04')
    expect(calcAge(defaultBirthDate(TODAY), TODAY)).toBe(30)
  })

  it('今天是闰日而 30 年前不是闰年时回退到 2 月 28 日', () => {
    expect(defaultBirthDate(new Date(2024, 1, 29))).toBe('1994-02-28')
  })
})

describe('isValidBirthDate', () => {
  it('区分闰年与平年的 2 月 29 日', () => {
    expect(isValidBirthDate('2024-02-29', TODAY)).toBe(true)
    expect(isValidBirthDate('2026-02-29', TODAY)).toBe(false)
  })
})

describe('validateProfileForm 表单校验', () => {
  it('出生日期、性别与常住地必填', () => {
    expect(validateProfileForm({ ...EMPTY_PROFILE_FORM }, TODAY)).toBe('请选择出生日期')
    expect(validateProfileForm({ ...EMPTY_PROFILE_FORM, birthDate: '1990-01-01' }, TODAY))
      .toBe('请选择性别')
    expect(validateProfileForm(
      { ...EMPTY_PROFILE_FORM, birthDate: '1990-01-01', gender: '男' }, TODAY,
    )).toBe('请填写常住地')
  })

  it('日期不合法时拦下', () => {
    expect(validateProfileForm(
      { ...EMPTY_PROFILE_FORM, birthDate: '1990-02-30', gender: '男', region: '广州' }, TODAY,
    )).toBe('出生日期不合法')
  })

  it('姓名与既往病史选填', () => {
    const err = validateProfileForm(
      { name: '', birthDate: '1990-01-01', gender: '男', region: '广州', history: '' }, TODAY,
    )
    expect(err).toBe('')
  })
})

describe('性别取值', () => {
  it('选项为男 / 女 / 不愿透露', () => {
    expect(GENDER_OPTIONS).toEqual(['男', '女', '不愿透露'])
  })

  it('男/女原样进 payload', () => {
    expect(toPayloadGender('男')).toBe('男')
    expect(toPayloadGender('女')).toBe('女')
  })

  it('未选与「不愿透露」都落到后端的 Unknown', () => {
    // 后端只认 男/女，其余一律 Unknown（不过滤问诊条目，宁可多问也不漏）
    expect(toPayloadGender('')).toBe('未知')
    expect(toPayloadGender('不愿透露')).toBe('未知')
    expect(toPayloadGender('保密')).toBe('未知')
  })

  it('回填时 Unknown 视为未选，让用户重新选', () => {
    expect(fromPayloadGender('女')).toBe('女')
    expect(fromPayloadGender('未知')).toBe('')
    expect(fromPayloadGender(undefined)).toBe('')
  })
})

describe('buildProfile 表单转档案', () => {
  it('派生周岁，姓名/病史留空则不落字段', () => {
    const p = buildProfile(
      { name: '', birthDate: '1990-01-01', gender: '男', region: '广州', history: '' }, TODAY,
    )
    expect(p.age).toBe(36)
    expect(p.name).toBeUndefined()
    expect(p.history).toBeUndefined()
    expect(p.region).toBe('广州')
    expect(p.gender).toBe('男')
  })

  it('未选性别与「不愿透露」都落到「未知」', () => {
    const base = { ...EMPTY_PROFILE_FORM, birthDate: '1990-01-01' }
    expect(buildProfile({ ...base, gender: '不愿透露' }, TODAY).gender).toBe('未知')
    expect(buildProfile({ ...base, gender: '' }, TODAY).gender).toBe('未知')
  })

  it('与 toProfileForm 可往返', () => {
    const form = {
      name: '张三', birthDate: '1990-01-01', gender: '男', region: '广州', history: '高血压',
    }
    expect(toProfileForm(buildProfile(form, TODAY))).toEqual(form)
  })
})

describe('buildOpeningMessages 首轮消息', () => {
  it('既往病史独立成条且排在主诉之前', () => {
    // 望/闻/切只取最后一条 user 消息做证据匹配，
    // 病史混进主诉会被当成当前症状；安全门则汇总全部 user 消息，放前面照样读得到
    expect(buildOpeningMessages('口苦口臭三天', '高血压 5 年，青霉素过敏')).toEqual([
      { role: 'user', content: '【既往病史】高血压 5 年，青霉素过敏' },
      { role: 'user', content: '口苦口臭三天' },
    ])
  })

  it('没有既往病史时只发主诉', () => {
    expect(buildOpeningMessages('咳嗽两天', '   ')).toEqual([
      { role: 'user', content: '咳嗽两天' },
    ])
  })
})

describe('describeProfile 摘要', () => {
  it('拼年龄、性别、常住地与既往病史', () => {
    expect(describeProfile(
      { gender: '男', birth_date: '1990-01-01', region: '广州', history: '高血压' }, TODAY,
    )).toBe('36岁 · 男 · 广州 · 既往：高血压')
  })

  it('性别未知时不显示这一项', () => {
    expect(describeProfile(
      { gender: '未知', birth_date: '1990-01-01', region: '广州', history: '高血压' }, TODAY,
    )).toBe('36岁 · 广州 · 既往：高血压')
  })

  it('旧档案只有年龄没有出生日期时仍显示', () => {
    expect(describeProfile({ gender: '未知', age: 36, region: '广州' }, TODAY)).toBe('36岁 · 广州')
  })

  it('空档案返回空串', () => {
    expect(describeProfile({ gender: '未知' }, TODAY)).toBe('')
  })
})

describe('normalizeMember 旧档案兼容', () => {
  it('旧 note 并入 history，旧 age 保留，旧性别保住', () => {
    const m = normalizeMember({
      id: 'm1', name: '父亲', relation: '父亲', note: '糖尿病',
      patient: { gender: '男', age: 62, height_cm: 170, weight_kg: 65 },
    })
    expect(m).not.toBeNull()
    expect(m!.patient.history).toBe('糖尿病')
    expect(m!.patient.age).toBe(62)
    // 性别是旧档案里唯一有价值的遗留字段，改版不能把它冲掉
    expect(m!.patient.gender).toBe('男')
    expect(m!.patient.name).toBeUndefined()
  })

  it('旧档案性别缺失时落到未知', () => {
    const m = normalizeMember({ id: 'm3', name: '其他', relation: '其他', patient: {} })
    expect(m!.patient.gender).toBe('未知')
  })

  it('新结构原样通过', () => {
    const m = normalizeMember({
      id: 'm2', name: '本人', relation: '本人',
      patient: { name: '张三', birth_date: '1990-01-01', region: '广州', history: '无' },
    })
    expect(m!.patient.birth_date).toBe('1990-01-01')
    expect(m!.patient.history).toBe('无')
  })

  it('脏数据返回 null而不是抛错', () => {
    expect(normalizeMember(null)).toBeNull()
    expect(normalizeMember('x')).toBeNull()
  })
})
