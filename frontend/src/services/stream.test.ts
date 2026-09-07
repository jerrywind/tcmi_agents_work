import { describe, it, expect } from 'vitest'
import {
  EVENTS, decodeUtf8, feedBytes, feedText, newByteBuffer, newSseBuffer, parseFrame,
} from './stream'

/**
 * SSE 帧解析器的回归测试。
 *
 * 为什么不测就算了：解析器错了**不会报错**——帧拼不出来，界面就是一直
 * 停在骨架上，看起来像「后端还在算」。这类静默失效只能在真机上看出来，
 * 而一次真机验证要 3–9 分钟，代价远高于几条单测。
 */

/** 把字符串编码成 UTF-8 字节（模拟网络分片） */
function enc(s: string): Uint8Array {
  return new TextEncoder().encode(s)
}

/** 构造一帧标准 SSE */
function frame(event: string, data: any): string {
  return `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`
}

describe('SSE 帧解析', () => {
  it('解析标准帧：event + data', () => {
    const ev = parseFrame('event: step_done\ndata: {"index":2}\n')
    expect(ev).not.toBeNull()
    expect(ev!.event).toBe('step_done')
    expect(ev!.data).toEqual({ index: 2 })
  })

  it('忽略注释帧（心跳 `: ping`）', () => {
    expect(parseFrame(': ping')).toBeNull()
  })

  it('data 里的中文与嵌套结构原样保留', () => {
    const ev = parseFrame(frame(EVENTS.STEP_DONE, { zh: '望诊', text: '舌淡红，苔薄白' }))
    expect(ev!.data.zh).toBe('望诊')
    expect(ev!.data.text).toBe('舌淡红，苔薄白')
  })

  it('单帧 JSON 损坏时不抛异常，返回 data=null（不掐断整条流）', () => {
    const ev = parseFrame('event: hello\ndata: {破损\n\n')
    expect(ev!.event).toBe('hello')
    expect(ev!.data).toBeNull()
  })

  it('CRLF 换行也能解析', () => {
    const ev = parseFrame('event: done\r\ndata: {"ok":1}')
    expect(ev!.event).toBe('done')
    expect(ev!.data).toEqual({ ok: 1 })
  })
})

describe('feedText：半帧要留到下一块', () => {
  it('一次喂入多帧，全部吐出', () => {
    const buf = newSseBuffer()
    const out = feedText(buf, frame('hello', { total: 7 }) + frame('step_start', { index: 0 }))
    expect(out).toHaveLength(2)
    expect(out[0].event).toBe('hello')
    expect(out[1].event).toBe('step_start')
    expect(buf.buf).toBe('')
  })

  it('喂入不完整的帧时不吐出，留在缓冲区', () => {
    const buf = newSseBuffer()
    const out = feedText(buf, 'event: hello\ndata: {"total":')
    expect(out).toHaveLength(0)
    expect(buf.buf).toBe('event: hello\ndata: {"total":')
  })

  it('半帧 + 后半帧拼起来后能正确吐出（网络分片的核心场景）', () => {
    const buf = newSseBuffer()
    const whole = frame('step_done', { index: 3, text: '脉浮数' })
    const cut = Math.floor(whole.length / 2)
    const a = feedText(buf, whole.slice(0, cut))
    const b = feedText(buf, whole.slice(cut))
    expect(a).toHaveLength(0)
    expect(b).toHaveLength(1)
    expect(b[0].data.text).toBe('脉浮数')
  })

  it('心跳帧混在业务帧中间不影响解析', () => {
    const buf = newSseBuffer()
    const out = feedText(buf, ': ping\n\n' + frame('loop', { converged: false }) + ': ping\n\n')
    expect(out).toHaveLength(1)
    expect(out[0].event).toBe('loop')
  })
})

describe('feedBytes：多字节汉字被切断也不能乱码', () => {
  it('按字节切分仍能正确解出中文正文', () => {
    const buf = newByteBuffer()
    const whole = frame('step_done', { zh: '望诊', text: '舌质淡红，苔薄白，脉细。' })
    const bytes = enc(whole)
    // 每 7 个字节切一片：必然切在汉字中间
    const out: any[] = []
    for (let i = 0; i < bytes.length; i += 7) {
      out.push(...feedBytes(buf, bytes.subarray(i, Math.min(i + 7, bytes.length))))
    }
    expect(out).toHaveLength(1)
    expect(out[0].data.zh).toBe('望诊')
    expect(out[0].data.text).toBe('舌质淡红，苔薄白，脉细。')
  })

  it('CRLF 结尾的帧不会被漏掉（只找 \\n\\n 会永远找不到 \\r\\n\\r\\n）', () => {
    const buf = newByteBuffer()
    const raw = 'event: done\r\ndata: {"ok":true}\r\n\r\n'
    const out = feedBytes(buf, enc(raw))
    expect(out).toHaveLength(1)
    expect(out[0].event).toBe('done')
  })

  it('一次喂入三帧，全部吐出且顺序正确', () => {
    const buf = newByteBuffer()
    const out = feedBytes(buf, enc(
      frame('hello', { total: 3 }) + frame('step_start', { index: 0 }) + frame('step_done', { index: 0 }),
    ))
    expect(out.map(e => e.event)).toEqual(['hello', 'step_start', 'step_done'])
  })
})

describe('decodeUtf8', () => {
  it('解出中文', () => {
    expect(decodeUtf8(enc('舌苔薄白'))).toBe('舌苔薄白')
  })

  it('解出 emoji（四字节，需代理对）', () => {
    expect(decodeUtf8(enc('⚠️ 提示'))).toBe('⚠️ 提示')
  })
})
