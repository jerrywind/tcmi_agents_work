import { describe, it, expect } from 'vitest'
import { charsPerFrame } from './useTypewriter'

/**
 * 打字机的速度策略。
 *
 * 为什么单独测它：固定「每帧 N 字」的写法在长文本下会**补播十几秒**，
 * 用户早就读完后面的内容了，动画还在追——这个退化不报错、不崩溃，
 * 只会让人觉得「这应用好卡」，属于典型的静默劣化。
 */
describe('charsPerFrame 自适应速度', () => {
  it('无积压时不吐字', () => {
    expect(charsPerFrame(0)).toBe(0)
  })

  it('积压极少时至少吐 MIN(2) 字，观感上仍在写', () => {
    expect(charsPerFrame(1)).toBe(1)
    expect(charsPerFrame(2)).toBe(2)
    expect(charsPerFrame(3)).toBeGreaterThanOrEqual(2)
  })

  it('积压 400 字时不会按固定 2 字/帧慢慢爬（600ms 内播完）', () => {
    const per = charsPerFrame(400, 16)
    // 600ms ≈ 37 帧；每帧至少要吐 400/37 ≈ 11 字
    expect(per).toBeGreaterThanOrEqual(11)
  })

  it('掉帧时按间隔补偿：间隔越长，单帧吐得越多', () => {
    const normal = charsPerFrame(600, 16)
    const janky = charsPerFrame(600, 160)
    expect(janky).toBeGreaterThan(normal)
  })

  it('永远不会超过积压量', () => {
    expect(charsPerFrame(5, 16)).toBeLessThanOrEqual(5)
    expect(charsPerFrame(1, 1000)).toBeLessThanOrEqual(1)
  })

  it('极长文本（2000 字）也在 600ms 内播完', () => {
    const per = charsPerFrame(2000, 16)
    // 600ms / 16ms = 37.5 帧（不取整），按实际帧数折算
    const frames = 600 / 16
    expect(per * frames).toBeGreaterThanOrEqual(2000)
  })
})
