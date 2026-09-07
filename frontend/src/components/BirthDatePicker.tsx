import { useEffect, useRef, useState } from 'react'
import Taro from '@tarojs/taro'
import { View, Text, ScrollView, Picker } from '@tarojs/components'
import {
  clampDay, daysInMonth, defaultBirthDate, formatBirthDate, parseBirthDate, parseYearOf,
} from '../utils/birthdate'
import { IS_WEAPP } from '../utils/platform'

/** 单个选项高度（px）。轮盘可见 5 项，选中项居中。 */
const ITEM_H = 44
/** 上下留白，让首尾项也能滚到中线。 */
const EDGE = (ITEM_H * 5 - ITEM_H) / 2

interface Props {
  /** 当前值 `YYYY-MM-DD`；空串表示未选 */
  value: string
  /** 可选最早日期 `YYYY-MM-DD` */
  start?: string
  /** 可选最晚日期 `YYYY-MM-DD` */
  end?: string
  onChange: (v: string) => void
}

/** 两端子组件拿到的都是已算好的起止日期。 */
interface InnerProps {
  value: string
  start: string
  end: string
  onChange: (v: string) => void
}

/**
 * 出生日期选择器。
 *
 * 不用 Taro 的 `<Picker mode='date'>`（H5 上是 transform 轮盘，背景会跟着滚、
 * 月日会留空白），H5 端自建三列原生滚动容器。小程序端反过来用原生 Picker，
 * 原因是那套滚轮依赖 `document` 与受控 `scrollTop` 的精确时序，只在 H5 验证过。
 *
 * 关键陷阱（与档案页日期选择器同源）：**绝不能静默写入默认值**。
 * 点开轮盘就算用户一格没滑、直接点「确定」，也会触发选择——所以初始轮盘位置
 * 只用于展示，`value` 为空时显示「请选择」，只有用户点「确定」才回写 `onChange`。
 */
export default function BirthDatePicker({ value, start = '1900-01-01', end, onChange }: Props) {
  const today = new Date()
  const endStr = end
    || `${today.getFullYear()}-${String(today.getMonth() + 1).padStart(2, '0')}-${String(today.getDate()).padStart(2, '0')}`

  // 端能力分派：滚轮只在 H5 上验证过，小程序端走原生控件。
  // 不做「同一份代码两端跑」，那一端必然是没验证过的那一端。
  if (IS_WEAPP) {
    return <NativeDatePicker value={value} start={start} end={endStr} onChange={onChange} />
  }
  return <WheelDatePicker value={value} start={start} end={endStr} onChange={onChange} />
}

/**
 * 小程序端：原生 `<Picker mode='date'>`。
 *
 * 原生 picker 有个已验证的坑：点开后一格不滑、直接点「确定」**也会**触发
 * onChange 写入定位值。因此定位值必须是「误触也无害」的值——今天往前 30 年
 * （而不是今天：曾用今天，静默写出 `age=0`，把人当成婴儿）。
 *
 * 即便误触写入的是无害值，那也等于替用户做了决定，所以再补两道：
 *   - 选中后 toast 告知选了什么，让人有机会发现「这不是我选的」；
 *   - 给「清除」入口，能退回「未选择」重新来过。
 */
function NativeDatePicker({ value, start, end, onChange }: InnerProps) {
  // 只用于把轮盘定位到合理位置，不是默认值：用户不操作就不会写进档案
  const [locator] = useState(() => defaultBirthDate())

  const pick = (v: string) => {
    // 与现值相同说明用户没真的改动，不必打扰
    if (!v || v === value) return
    onChange(v)
    Taro.showToast({ title: `已选 ${v}`, icon: 'none' })
  }

  return (
    <View className='form-row'>
      <Text className='form-label'>出生日期</Text>
      <Picker className='form-input' mode='date' start={start} end={end}
        value={value || locator}
        onChange={e => pick(e.detail.value as string)}>
        <Text className={value ? '' : 'placeholder'}>{value || '请选择'}</Text>
      </Picker>
      {value
        ? <Text className='form-clear' onClick={() => onChange('')}>清除</Text>
        : null}
    </View>
  )
}

/**
 * H5 端：底部弹层 + 年/月/日三列滚动。
 *
 * 三列都是原生滚动容器：
 *   - `overscroll-behavior: contain` 把滚动锁在列内，背景整页不再上移下拉；
 *   - 我们自己渲染每一项，不会留下空白格。
 *
 * 滚动定位用**受控 `scrollTop`**（= 选中索引 × 单项高），配合 `.bdp-indicator`
 * 指示线居中；不用 `scrollIntoView`——浏览器会把它顶到列顶，与居中指示线错位。
 */
function WheelDatePicker({ value, start, end, onChange }: InnerProps) {
  const today = new Date()
  const yearStart = parseYearOf(start, 1900)
  const yearEnd = parseYearOf(end, today.getFullYear())
  const yearRange: number[] = []
  for (let y = yearStart; y <= yearEnd; y++) yearRange.push(y)

  const [open, setOpen] = useState(false)
  // 当前选中索引（年/月/日）。月、日从 1 开始，索引 = 值 - 1。
  const [selY, setSelY] = useState(0)
  const [selM, setSelM] = useState(0)
  const [selD, setSelD] = useState(0)
  // 各列滚动位置（受控），打开时定位到初始索引。
  const [topY, setTopY] = useState(0)
  const [topM, setTopM] = useState(0)
  const [topD, setTopD] = useState(0)

  /** 打开弹层：由 value（优先）或默认定位日期算出初始位置。 */
  const openSheet = () => {
    const init = parseBirthDate(value) ?? parseBirthDate(defaultBirthDate(today))!
    const yIdx = Math.max(0, yearRange.indexOf(init.y))
    const mIdx = init.m - 1
    const dIdx = init.d - 1
    setSelY(yIdx)
    setSelM(mIdx)
    setSelD(dIdx)
    setTopY(yIdx * ITEM_H)
    setTopM(mIdx * ITEM_H)
    setTopD(dIdx * ITEM_H)
    setOpen(true)
  }

  const closeSheet = () => setOpen(false)

  const confirm = () => {
    const year = yearRange[selY]
    const month = selM + 1
    const day = clampDay(year, month, selD + 1)
    onChange(formatBirthDate(year, month, day))
    closeSheet()
  }

  const monthLen = 12
  const dayLen = daysInMonth(yearRange[selY], selM + 1)
  const months = Array.from({ length: monthLen }, (_, i) => i + 1)
  const days = Array.from({ length: dayLen }, (_, i) => i + 1)

  /**
   * 滚动定位关键陷阱（踩过：滑一下弹回原位、日期改不了）：
   * 受控 `scrollTop`（= 选中索引 × 单项高）只在「打开时」和「停滚吸附」时被设置。
   * 拖拽过程中**绝不能**调用 setState——否则每帧重渲染会让 Taro H5 的 <ScrollView> 把
   * scrollTop 重设回受控值（初始/上次快照），表现就是「滑一下又弹回原位」。
   * 所以 onScroll 只记录最近滚动距离，停滚 140ms 后才一次性提交高亮 + 吸附到最近项。
   */
  const lastScroll = useRef<{ y: number; m: number; d: number }>({ y: 0, m: 0, d: 0 })
  const snapTimers = useRef<{ y: ReturnType<typeof setTimeout> | null; m: ReturnType<typeof setTimeout> | null; d: ReturnType<typeof setTimeout> | null }>({ y: null, m: null, d: null })

  /** 停滚后提交：更新选中索引（高亮 + 确定回写）并按受控 scrollTop 平滑吸附到最近项 */
  const applyIdx = (col: 'y' | 'm' | 'd', idx: number) => {
    if (col === 'y') { setSelY(idx); setTopY(idx * ITEM_H) }
    else if (col === 'm') { setSelM(idx); setTopM(idx * ITEM_H) }
    else { setSelD(idx); setTopD(idx * ITEM_H) }
    // 年/月变化可能让日越界，钳制后把日列吸附到合法位置（居中）
    if (col !== 'd') {
      const year = col === 'y' ? yearRange[idx] : yearRange[selY]
      const month = col === 'y' ? idx + 1 : selM + 1
      const maxDay = daysInMonth(year, month)
      if (selD + 1 > maxDay) {
        const dIdx = maxDay - 1
        setSelD(dIdx)
        setTopD(dIdx * ITEM_H)
      }
    }
  }

  const onColScroll = (col: 'y' | 'm' | 'd', scrollTop: number, len: number) => {
    lastScroll.current[col] = scrollTop
    const t = snapTimers.current[col]
    if (t) clearTimeout(t)
    snapTimers.current[col] = setTimeout(() => {
      const st = lastScroll.current[col]
      const idx = Math.min(Math.max(Math.round(st / ITEM_H), 0), len - 1)
      applyIdx(col, idx)
    }, 140)
  }

  // 打开时把 body 滚动锁住（兜底：各列 ScrollView 已 overscroll-behavior: contain）
  useEffect(() => {
    // 小程序端没有 `document`，裸访问会直接 ReferenceError。
    // 本组件在该端走 NativeDatePicker 分支，这里是双保险。
    if (!open || typeof document === 'undefined') return
    const prev = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    return () => { document.body.style.overflow = prev }
  }, [open])

  const renderColumn = (
    col: 'y' | 'm' | 'd',
    items: number[],
    selIdx: number,
    top: number,
    unit: string,
  ) => (
    // 包一层 View 处理 flex 尺寸：Taro H5 的 <ScrollView> 渲染成 <taro-scroll-view-core>，
    // host 元素不会响应 flex:1，会按内容宽度坍缩，列与列无法并排。
    <View className='bdp-col-wrap' style={{ flex: 1, height: '100%', overflow: 'hidden' }}>
      <ScrollView
        className='bdp-col'
        scrollY
        scrollWithAnimation
        scrollTop={top}
        style={{ width: '100%', height: '100%', overscrollBehavior: 'contain', touchAction: 'pan-y', WebkitOverflowScrolling: 'touch' }}
        onScroll={e => onColScroll(col, e.detail.scrollTop, items.length)}
      >
        <View style={{ height: EDGE }} />
        {items.map((it, i) => (
          <View
            key={i}
            className={`bdp-item ${i === selIdx ? 'selected' : ''}`}
            style={{
              height: '44px', lineHeight: '44px', textAlign: 'center', fontSize: '30px',
              color: i === selIdx ? '#2a6f4e' : '#333',
              fontWeight: i === selIdx ? 600 : 400,
            }}
          >
            {it}
            {unit}
          </View>
        ))}
        <View style={{ height: EDGE }} />
      </ScrollView>
    </View>
  )

  return (
    <>
      <View className='form-row' onClick={openSheet}>
        <Text className='form-label'>出生日期</Text>
        <Text className={`form-input ${value ? '' : 'placeholder'}`}>{value || '请选择'}</Text>
      </View>

      {open && (
        <View className='bdp-mask' onClick={closeSheet}>
          <View className='bdp-sheet' onClick={e => e.stopPropagation()}>
            <View className='bdp-hd'>
              <Text className='bdp-action cancel' onClick={closeSheet}>取消</Text>
              <Text className='bdp-action ok' onClick={confirm}>确定</Text>
            </View>
            <View
              className='bdp-bd'
              style={{ position: 'relative', display: 'flex', height: '220px', overflow: 'hidden' }}
            >
              <View
                className='bdp-indicator'
                style={{
                  position: 'absolute', left: 0, right: 0, top: '88px',
                  height: '44px', borderTop: '1px solid #e5e5e5', borderBottom: '1px solid #e5e5e5',
                  background: 'rgba(42,111,78,0.06)', pointerEvents: 'none',
                }}
              />
              {renderColumn('y', yearRange, selY, topY, '年')}
              {renderColumn('m', months, selM, topM, '月')}
              {renderColumn('d', days, selD, topD, '日')}
            </View>
          </View>
        </View>
      )}
    </>
  )
}
