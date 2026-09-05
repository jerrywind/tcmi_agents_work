import { describe, it, expect } from 'vitest'
import {
  clampDay, daysInMonth, defaultBirthDate, formatBirthDate, pad2, parseBirthDate, parseYearOf,
} from './birthdate'

describe('pad2 两位补零', () => {
  it('个位补零、十位原样', () => {
    expect(pad2(0)).toBe('00')
    expect(pad2(9)).toBe('09')
    expect(pad2(12)).toBe('12')
  })
})

describe('daysInMonth 月天数', () => {
  it('小月 30 天、大月 31 天', () => {
    expect(daysInMonth(2026, 4)).toBe(30)
    expect(daysInMonth(2026, 1)).toBe(31)
  })
  it('闰年 2 月 29、平年 2 月 28', () => {
    expect(daysInMonth(2024, 2)).toBe(29)
    expect(daysInMonth(2026, 2)).toBe(28)
    expect(daysInMonth(1900, 2)).toBe(28) // 整百年非闰
    expect(daysInMonth(2000, 2)).toBe(29) // 400 倍数闰
  })
})

describe('clampDay 跨月钳制', () => {
  it('1 月 31 日改到 2 月应落到 28/29', () => {
    expect(clampDay(2026, 2, 31)).toBe(28)
    expect(clampDay(2024, 2, 31)).toBe(29)
  })
  it('下限不低于 1', () => {
    expect(clampDay(2026, 2, 0)).toBe(1)
  })
})

describe('formatBirthDate / parseBirthDate 往返', () => {
  it('格式化严格补零', () => {
    expect(formatBirthDate(2026, 3, 5)).toBe('2026-03-05')
  })
  it('能解析严格格式', () => {
    expect(parseBirthDate('2026-03-05')).toEqual({ y: 2026, m: 3, d: 5 })
  })
  it('只认严格补零格式，其余返回 null', () => {
    expect(parseBirthDate('2026-3-5')).toBeNull()   // 没补零
    expect(parseBirthDate('2026/03/05')).toBeNull() // 斜杠
    expect(parseBirthDate('2026-13-01')).toBeNull() // 月越界
    expect(parseBirthDate('2026-02-30')).toBeNull() // 日越界
    expect(parseBirthDate('')).toBeNull()
  })
  it('格式化结果能被自身解析（选择器吐出的值可被档案校验认下）', () => {
    const s = formatBirthDate(2000, 2, 29)
    expect(parseBirthDate(s)).toEqual({ y: 2000, m: 2, d: 29 })
  })
})

describe('parseYearOf', () => {
  it('非法值返回兜底年份', () => {
    expect(parseYearOf('', 1990)).toBe(1990)
    expect(parseYearOf('乱码', 1990)).toBe(1990)
  })
  it('合法值取年份', () => {
    expect(parseYearOf('1996-09-04', 1990)).toBe(1996)
  })
})

describe('defaultBirthDate 选择器定位日期', () => {
  const TODAY = new Date(2026, 8, 4) // 2026-09-04
  it('定位到今天往前 30 年，而不是今天', () => {
    // 点开再点「确定」会静默触发选择，定位到今天会写出 age=0 把成年人当婴儿
    expect(defaultBirthDate(TODAY)).toBe('1996-09-04')
  })
  it('今天是闰日而 30 年前不是闰年时回退到 2 月 28 日', () => {
    expect(defaultBirthDate(new Date(2024, 1, 29))).toBe('1994-02-28')
  })
  it('吐出的定位日期可被 parseBirthDate 认下', () => {
    expect(parseBirthDate(defaultBirthDate(TODAY))).not.toBeNull()
  })
})
