import { useEffect, useRef, useState } from 'react'

/**
 * 打字机：把「整段突然出现」变成「逐字写出来」。
 *
 * ## 为什么需要它
 *
 * 流式把等待切成了一段一段，但每一段仍是**整块**到达的：一个新段落
 * 「啪」地出现在屏幕上，视觉上仍然突兀。打字机让新内容有生长感，
 * 也让「系统正在输出」这件事变得可见。
 *
 * ## 关键设计：自适应速度
 *
 * 固定「每帧 N 字」是错的：后端一次吐 800 字时，按 N=2 要播 400 帧 ≈ 6.7 秒，
 * 用户早把后面读完了，动画还在补播——反而更慢。
 * 这里按**积压量**决定每帧字数：目标是任何积压都在 ~600ms 内播完。
 *
 * ## 用户可以打断
 *
 * `flush()` 立即补齐全部文本。滚动、点击、切步骤时都应调用：
 * 动画是给等待用的，不是用来挡住用户的。
 */

/** 单帧基准：积压很小时也至少出这么多字，保证观感是「在写」而不是「卡住」 */
const MIN_CHARS_PER_FRAME = 2

/** 目标：任何积压都在这个时间内播完（毫秒） */
const CATCHUP_MS = 600

/** 约 60fps */
const FRAME_MS = 16

/**
 * 计算下一帧应该吐多少字（纯函数，便于单测）。
 *
 * @param backlog 还有多少字没显示
 * @param elapsed 距上次出字的间隔（毫秒，用于掉帧补偿）
 */
export function charsPerFrame(backlog: number, elapsed = FRAME_MS): number {
  if (backlog <= 0) return 0
  // 掉帧时按比例多吐，避免长文本在低端机上永远追不上
  const frames = Math.max(1, CATCHUP_MS / Math.max(elapsed, FRAME_MS))
  const adaptive = Math.ceil(backlog / frames)
  return Math.min(backlog, Math.max(MIN_CHARS_PER_FRAME, adaptive))
}

export interface TypewriterOptions {
  /** 每帧字数上限（想更慢就调小；默认按积压自适应） */
  maxCharsPerFrame?: number
  /** 关闭打字机（例如用户开了「减少动效」） */
  disabled?: boolean
}

export interface TypewriterResult {
  /** 当前应显示的文本 */
  shown: string
  /** 是否已全部显示 */
  done: boolean
  /** 立即显示全部 */
  flush: () => void
}

export function useTypewriter(text: string, opts: TypewriterOptions = {}): TypewriterResult {
  const disabled = !!opts.disabled
  const maxChars = opts.maxCharsPerFrame ?? Number.MAX_SAFE_INTEGER
  // 长度用 ref 驱动动画，state 只用于触发重渲染：
  // 否则 `requestAnimationFrame` 里读到的 `shown` 永远是闭包里的旧值。
  const lenRef = useRef(0)
  const [len, setLen] = useState(0)
  const raf = useRef<number | null>(null)
  const last = useRef(0)
  const prevText = useRef('')
  const target = text.length

  // 文本被**替换**成一段全新的（不是在原文本上追加）时从头播；
  // 追加（流式场景的常态）则保留已播进度，不重头再来。
  useEffect(() => {
    if (!text.startsWith(prevText.current)) {
      lenRef.current = 0
      setLen(0)
    }
    prevText.current = text
  }, [text])

  const stop = () => {
    if (raf.current !== null) {
      cancelAnimationFrame(raf.current)
      raf.current = null
    }
    last.current = 0
  }

  useEffect(() => {
    if (disabled) {
      lenRef.current = target
      setLen(target)
      return
    }
    if (lenRef.current >= target) return

    const tick = (now: number) => {
      const elapsed = last.current ? now - last.current : FRAME_MS
      last.current = now
      const backlog = target - lenRef.current
      if (backlog <= 0) {
        // 播完了就停，别让 rAF 空转烧 CPU
        raf.current = null
        return
      }
      lenRef.current += Math.min(maxChars, charsPerFrame(backlog, elapsed))
      setLen(lenRef.current)
      raf.current = requestAnimationFrame(tick)
    }
    raf.current = requestAnimationFrame(tick)
    return stop
  }, [target, disabled, maxChars])

  useEffect(() => stop, [])

  return {
    shown: disabled ? text : text.slice(0, len),
    done: disabled || lenRef.current >= target,
    flush: () => {
      stop()
      lenRef.current = target
      setLen(target)
    },
  }
}
