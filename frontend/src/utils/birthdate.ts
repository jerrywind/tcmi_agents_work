/**
 * 出生日期选择器的纯逻辑。
 *
 * 为什么不用 Taro 自带的 `<Picker mode='date'>`：
 * Taro 4 在 H5 端把日期 Picker 实现成 Stencil 自定义元素（`<taro-picker-core>`），
 * 用 CSS transform 模拟轮盘滚动——在部分手机浏览器上有两个修不掉的问题：
 *   1) 轮盘的 touchmove 会冒泡到 document，导致背景整页跟着上移/下拉；
 *   2) transform 轮盘在滚动/重绘时留下空白项，年/月/日（尤其月、日）显示不全。
 * 应用层抓不到它的内部事件，也改不了它的渲染，只能换掉。
 *
 * 这里把"索引 ↔ 日期值"的换算全部抽成纯函数，单独单测；
 * 选择器组件（BirthDatePicker）只负责滚动与渲染，不掺业务逻辑。
 */

const pad = (n: number) => String(n).padStart(2, '0')

/** 两位补零：`9` -> `"09"`。 */
export function pad2(n: number): string {
  return pad(n)
}

/** 某年某月有多少天（处理了平年/闰年二月）。 */
export function daysInMonth(year: number, month: number): number {
  if (month === 4 || month === 6 || month === 9 || month === 11) return 30
  if (month === 2) {
    if ((year % 4 === 0 && year % 100 !== 0) || year % 400 === 0) return 29
    return 28
  }
  return 31
}

/**
 * 把日钳制进"当年当月"合法范围。
 *
 * 用户先选了 1 月 31 日，再改月份到 2 月——2 月没有 31 日，
 * 必须落到当月最大日，否则会攒出一个"2 月 31 日"这种非法日期。
 */
export function clampDay(year: number, month: number, day: number): number {
  return Math.min(Math.max(day, 1), daysInMonth(year, month))
}

/** 由年/月/日拼成 `YYYY-MM-DD`（两位补零）。 */
export function formatBirthDate(year: number, month: number, day: number): string {
  return `${year}-${pad2(month)}-${pad2(day)}`
}

/**
 * 解析 `YYYY-MM-DD`。
 *
 * 只接受严格补零的格式；与 `profile.ts` 里 `isValidBirthDate` 的口径保持一致，
 * 选择器吐出去的值必须能被档案校验原样认下。解析不出返回 `null`。
 */
export function parseBirthDate(value: string | undefined): { y: number; m: number; d: number } | null {
  const s = (value || '').trim()
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s)
  if (!m) return null
  const y = Number(m[1])
  const mo = Number(m[2])
  const d = Number(m[3])
  if (mo < 1 || mo > 12) return null
  if (d < 1 || d > daysInMonth(y, mo)) return null
  return { y, m: mo, d }
}

/** 从 `YYYY-MM-DD` 取年份（非法返回兜底年）。 */
export function parseYearOf(value: string | undefined, fallback: number): number {
  const p = parseBirthDate(value)
  return p ? p.y : fallback
}

/**
 * 日期选择器打开时定位到的默认日期：**今天往前 30 年**。
 *
 * 不能默认定位到今天：选择器只要点开再点「确定」就会触发选择，哪怕一格没滑——
 * 那会静默写入「今天」，算出来 age=0，把成年人当成新生儿。
 * 定位到一个成年人常见的年份，误触的最坏结果也只是一个看得见、可被改的日期。
 * 今天是 2 月 29 日、而 30 年前不是闰年时回退一天。
 */
export function defaultBirthDate(today: Date = new Date()): string {
  const y = today.getFullYear() - 30
  const m = today.getMonth() + 1
  const d = m === 2 && today.getDate() === 29 && daysInMonth(y, 2) === 28 ? 28 : today.getDate()
  return `${y}-${pad(m)}-${pad(d)}`
}
