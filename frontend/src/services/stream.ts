import Taro from '@tarojs/taro'
import { IS_WEAPP } from '../utils/platform'
import { HARNESS_API_PREFIX, HARNESS_BASE_URL } from './harness'
import type { HarnessMessage } from './harness'

/**
 * harness 流式诊断客户端（`POST /chat/stream`，SSE）。
 *
 * ## 为什么另开一个端点而不是给 `/chat` 加个开关
 *
 * `/chat` 是「跑完 10 步一次性返回」，标准档实测 200–530 秒，
 * 期间前端只有一句 loading。流式把这段时间切成**事件流**逐步推：
 *
 * ```text
 * hello（步骤计划）→ step_start → step_done ×N → loop → confidence → summary → done
 * ```
 *
 * 总耗时不变，但用户**第一秒**就看到步骤骨架，之后每完成一步就有新内容。
 *
 * ## 帧格式
 *
 * ```text
 * event: <name>\n
 * data: <单行 JSON>\n
 * \n
 * ```
 *
 * 与标准 SSE 一致，故 H5 与小程序可以共用同一套解析器
 * （小程序没有 `EventSource`，只能拿分片自己拼）。
 *
 * ## 为什么不用 `EventSource`
 *
 * 它只能发 GET，而我们的 `messages` + 舌象图 base64 必须走 POST。
 */

/** 一条 SSE 事件 */
export interface StreamEvent {
  /** 事件名（见 `EVENTS`） */
  event: string
  /** 事件负载（已解析的 JSON；解析失败时为 null） */
  data: any
}

/** 后端下发的事件名（与 `server/harness/src/stream.rs` 一一对应） */
export const EVENTS = {
  HELLO: 'hello',
  STEP_START: 'step_start',
  /** 单步正文的 token 增量（`index` + `delta`），边生成边推 */
  STEP_DELTA: 'step_delta',
  /**
   * 该步的 LLM 调用失败、准备重试（见 `DeltaSink::emit_retry`）。
   *
   * 两层含义：① 让用户看到「重试中」而不是干等——单次超时 120s × 3 次尝试
   * 就是 6 分钟无任何内容；② **之前推的 delta 作废**，必须清空后重新累积，
   * 否则重试会把正文从头再推一遍，累加出两份重复内容。
   */
  STEP_RETRY: 'step_retry',
  STEP_DONE: 'step_done',
  STEP_FAIL: 'step_fail',
  RED_FLAG: 'red_flag',
  BLOCKED: 'blocked',
  SKIPPED: 'skipped',
  LOOP: 'loop',
  CONFIDENCE: 'confidence',
  SUMMARY: 'summary',
  DONE: 'done',
  ERROR: 'error',
} as const

/** 单个步骤的状态（前端据此渲染胶囊） */
export type StepState = 'pending' | 'running' | 'done' | 'failed' | 'skipped'

/** 步骤计划项（`hello` 事件的 `plan` 元素） */
export interface PlanStep {
  index: number
  capability: string
  zh: string
  phase: 'collection' | 'diagnosis' | 'safety' | 'treatment'
}

export interface StreamChatParams {
  messages: HarnessMessage[]
  payload?: Record<string, any>
  /** 每来一帧回调一次 */
  onEvent: (e: StreamEvent) => void
  /** 连接/解析失败 */
  onError?: (e: Error) => void
  /** 流正常结束（收到 `done` 或通道关闭） */
  onDone?: () => void
}

export interface StreamHandle {
  /** 取消：关闭连接并停止回调 */
  cancel: () => void
}

// ---------------------------------------------------------------- 帧解析

/**
 * UTF-8 解码。
 *
 * 优先用 `TextDecoder`（浏览器与新版小程序基础库都有）；
 * 没有时退回手写实现——小程序低版本基础库确实没有 `TextDecoder`，
 * 而中文正文是 UTF-8，解码错一个字节整句就变乱码。
 */
export function decodeUtf8(bytes: Uint8Array): string {
  const TD = (globalThis as any).TextDecoder
  if (typeof TD === 'function') {
    try {
      return new TD('utf-8').decode(bytes)
    } catch {
      // 落到手写实现
    }
  }
  return decodeUtf8Manual(bytes)
}

/** 手写 UTF-8 解码（无 `TextDecoder` 时的兜底） */
function decodeUtf8Manual(bytes: Uint8Array): string {
  let out = ''
  let i = 0
  while (i < bytes.length) {
    const b = bytes[i]
    let cp = 0
    let extra = 0
    if (b < 0x80) {
      cp = b
      extra = 0
    } else if (b >= 0xc0 && b < 0xe0) {
      cp = b & 0x1f
      extra = 1
    } else if (b >= 0xe0 && b < 0xf0) {
      cp = b & 0x0f
      extra = 2
    } else if (b >= 0xf0 && b < 0xf8) {
      cp = b & 0x07
      extra = 3
    } else {
      out += '\ufffd'
      i += 1
      continue
    }
    if (i + extra >= bytes.length) {
      // 字节被截断：交给下一块补齐（正常情况下不会走到，帧是完整才解码的）
      out += '\ufffd'
      break
    }
    let ok = true
    for (let j = 1; j <= extra; j += 1) {
      const c = bytes[i + j]
      if ((c & 0xc0) !== 0x80) {
        ok = false
        break
      }
      cp = (cp << 6) | (c & 0x3f)
    }
    if (!ok) {
      out += '\ufffd'
      i += 1
      continue
    }
    i += extra + 1
    if (cp > 0xffff) {
      const v = cp - 0x10000
      out += String.fromCharCode(0xd800 + (v >> 10), 0xdc00 + (v & 0x3ff))
    } else {
      out += String.fromCharCode(cp)
    }
  }
  return out
}

/**
 * 追加一段文本，吐出其中**完整**的 SSE 帧；剩下的半帧留在缓冲区里。
 *
 * 为什么要「留半帧」：网络分片可以在任意字节处切断，中文又是多字节，
 * 直接对单个 chunk 做 `JSON.parse` 必然炸。这是流式客户端最容易写错的地方。
 *
 * 纯函数、不依赖任何平台 API，故可直接单测（见 `stream.test.ts`）。
 */
export function feedText(parser: SseBuffer, text: string): StreamEvent[] {
  parser.buf += text
  const out: StreamEvent[] = []
  // 帧以空行结尾：`\n\n`（或 CRLF 的 `\r\n\r\n`）
  let sep = -1
  while ((sep = findFrameEnd(parser.buf)) >= 0) {
    const raw = parser.buf.slice(0, sep)
    // sep 指向空行的第一个字符，跳过空行本身（可能是 \r\n\r\n）
    const rest = parser.buf.slice(sep)
    const skip = rest.startsWith('\r\n\r\n') ? 4 : 2
    parser.buf = rest.slice(skip)
    const ev = parseFrame(raw)
    if (ev) out.push(ev)
  }
  return out
}

/** SSE 帧缓冲区（`feedText` 的可变状态） */
export interface SseBuffer {
  buf: string
}

export function newSseBuffer(): SseBuffer {
  return { buf: '' }
}

/** 找空行分隔符的位置；找不到返回 -1 */
function findFrameEnd(s: string): number {
  const a = s.indexOf('\n\n')
  const b = s.indexOf('\r\n\r\n')
  if (a < 0) return b
  if (b < 0) return a
  return Math.min(a, b)
}

/** 解析单个帧（不含结尾空行）；注释帧（`: ping`）返回 null */
export function parseFrame(raw: string): StreamEvent | null {
  let event = 'message'
  let data = ''
  for (const line of raw.split(/\r?\n/)) {
    if (line === '' || line.startsWith(':')) continue // 注释/心跳
    const idx = line.indexOf(':')
    if (idx < 0) continue
    const key = line.slice(0, idx)
    // `data: {...}` 的冒号后惯例带一个空格
    let val = line.slice(idx + 1)
    if (val.startsWith(' ')) val = val.slice(1)
    if (key === 'event') event = val
    else if (key === 'data') data += val
  }
  if (!data) return null
  let parsed: any = null
  try {
    parsed = JSON.parse(data)
  } catch {
    // 单帧解析失败不该掐断整条流：丢掉这一帧，后面的照常处理
    parsed = null
  }
  return { event, data: parsed }
}

/**
 * 追加一段**字节**，按帧边界切出完整帧后再解码。
 *
 * 与 `feedText` 的区别：这里保证「只在帧完整时才解码」，
 * 因此不会出现多字节汉字被切成两半。
 */
export function feedBytes(parser: ByteBuffer, chunk: Uint8Array): StreamEvent[] {
  const merged = concat(parser.pending, chunk)
  const out: StreamEvent[] = []
  let start = 0
  for (;;) {
    const sep = findFrameSep(merged, start)
    if (!sep) break
    const raw = decodeUtf8(merged.subarray(start, sep.at))
    start = sep.at + sep.skip
    const ev = parseFrame(raw)
    if (ev) out.push(ev)
  }
  parser.pending = merged.subarray(start)
  return out
}

export interface ByteBuffer {
  pending: Uint8Array
}

export function newByteBuffer(): ByteBuffer {
  return { pending: new Uint8Array(0) }
}

/**
 * 在字节数组里找帧分隔符：`\n\n`（LF）或 `\r\n\r\n`（CRLF）。
 *
 * 两种都要判：SSE 规范允许 CRLF，而只找 `\n\n` 在 CRLF 下会**永远找不到**
 * （`\r\n\r\n` 里两个 `\n` 中间隔着 `\r`）——帧会一直堆在缓冲区里，
 * 表现为「流式一动不动却也没报错」，属于最难查的一类静默失效。
 */
function findFrameSep(b: Uint8Array, from: number): { at: number; skip: number } | null {
  for (let i = Math.max(0, from); i < b.length; i += 1) {
    if (
      b[i] === 0x0d && i + 3 < b.length &&
      b[i + 1] === 0x0a && b[i + 2] === 0x0d && b[i + 3] === 0x0a
    ) {
      return { at: i, skip: 4 }
    }
    if (b[i] === 0x0a && i + 1 < b.length && b[i + 1] === 0x0a) {
      return { at: i, skip: 2 }
    }
  }
  return null
}

function concat(a: Uint8Array, b: Uint8Array): Uint8Array {
  if (a.length === 0) return b
  const out = new Uint8Array(a.length + b.length)
  out.set(a, 0)
  out.set(b, a.length)
  return out
}

// ---------------------------------------------------------------- 请求

function streamUrl(): string {
  return `${HARNESS_BASE_URL}${HARNESS_API_PREFIX}/chat/stream`
}

/**
 * 首帧看门狗窗口：连接发起后若这么久收不到**任何**事件，判定流式链路不通。
 *
 * `hello`（步骤计划）是后端 resolve_order 的纯函数产物，收到请求后毫秒级即发出，
 * 所以只要连接是通的，正常不会超过几秒。45 秒只可能出现在：SSE 被中间层整体
 * 缓冲、后端在读完请求体前就没起来、或链路被掐住——这些场景里用户看到的
 * 只有一直走的秒表。触发后 abort 连接并报错，由上层（consult）走既有降级路径：
 * 一步未收 → 自动改用非流式 `/chat`，用户不至于对着秒表白等 600 秒。
 */
export const FIRST_FRAME_TIMEOUT_MS = 45_000

const FIRST_FRAME_TIMEOUT_MSG = '流式通道 45 秒无响应，已切换普通模式'

/**
 * 发起一次流式问诊。
 *
 * H5 走 `fetch` + `ReadableStream`；小程序走 `Taro.request` 的
 * `enableChunked`（它拿不到 `EventSource`，只能自己拼分片）。
 *
 * 两条路径解析出的事件完全一致，上层无需区分端。
 */
export function streamChat(p: StreamChatParams): StreamHandle {
  return IS_WEAPP ? streamChatWeapp(p) : streamChatH5(p)
}

/** H5：`fetch` + `ReadableStream` */
function streamChatH5(p: StreamChatParams): StreamHandle {
  const controller = new AbortController()
  let closed = false
  let firstFrameTimer: ReturnType<typeof setTimeout> | null = null
  const clearFirstFrameTimer = () => {
    if (firstFrameTimer) {
      clearTimeout(firstFrameTimer)
      firstFrameTimer = null
    }
  }
  const finish = (err?: Error) => {
    clearFirstFrameTimer()
    if (closed) return
    closed = true
    if (err) p.onError?.(err)
    else p.onDone?.()
  }

  const run = async () => {
    // 看门狗先于 fetch 启动：覆盖「请求体上传中 / 后端无响应 / 帧被缓冲」整段。
    // 收到任意一帧即视为链路通，在事件循环里清除。
    firstFrameTimer = setTimeout(() => {
      clearFirstFrameTimer()
      if (closed) return
      try {
        controller.abort()
      } catch {
        // 已结束的流再 abort 会抛，忽略
      }
      finish(new Error(FIRST_FRAME_TIMEOUT_MSG))
    }, FIRST_FRAME_TIMEOUT_MS)
    try {
      const res = await fetch(streamUrl(), {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ messages: p.messages, payload: p.payload ?? {} }),
        signal: controller.signal,
      })
      if (!res.ok || !res.body) {
        finish(new Error(`流式请求失败：HTTP ${res.status}`))
        return
      }
      const reader = res.body.getReader()
      const buf = newByteBuffer()
      for (;;) {
        const { done, value } = await reader.read()
        if (done) break
        if (!value) continue
        for (const ev of feedBytes(buf, value)) {
          // 有事件到达即链路通畅：关掉首帧看门狗
          clearFirstFrameTimer()
          if (ev.event === EVENTS.DONE || ev.event === EVENTS.ERROR) {
            p.onEvent(ev)
            finish(ev.event === EVENTS.ERROR ? new Error(ev.data?.message || '问诊失败') : undefined)
            return
          }
          p.onEvent(ev)
        }
      }
      // 流结束却没收到 `done`：多半是后端崩了或连接被中间层掐断
      finish(closed ? undefined : new Error('流式连接中断（未收到结束帧）'))
    } catch (e: any) {
      if (e?.name === 'AbortError') {
        finish()
        return
      }
      finish(new Error(e?.message || '网络异常'))
    }
  }
  void run()

  return {
    cancel: () => {
      closed = true
      try {
        controller.abort()
      } catch {
        // 已结束的流再 abort 会抛，忽略
      }
    },
  }
}

/**
 * 小程序：`Taro.request({ enableChunked: true })`。
 *
 * ⚠️ **小程序端的 60 秒单次请求上限不会因流式而消失**：它能让用户早看到
 * 前几步，但仍然拿不到完整结论。真正的解法是任务化（提交 + 轮询），
 * 另立条目；这里只是让「超时前也能看到点东西」。
 */
function streamChatWeapp(p: StreamChatParams): StreamHandle {
  let closed = false
  const buf = newByteBuffer()
  const finish = (err?: Error) => {
    if (closed) return
    closed = true
    if (err) p.onError?.(err)
    else p.onDone?.()
  }
  const handle = (ev: StreamEvent) => {
    if (ev.event === EVENTS.DONE) {
      p.onEvent(ev)
      finish()
      return
    }
    if (ev.event === EVENTS.ERROR) {
      p.onEvent(ev)
      finish(new Error(ev.data?.message || '问诊失败'))
      return
    }
    p.onEvent(ev)
  }

  // `enableChunked` / `onChunkReceived` 不在 Taro 的公开类型里（小程序专有），
  // 故此处做一次收窄的断言，避免为它放宽整个 request 的类型。
  const req = Taro.request as unknown as (o: any) => any
  const task = req({
    url: streamUrl(),
    method: 'POST',
    header: { 'Content-Type': 'application/json' },
    data: { messages: p.messages, payload: p.payload ?? {} },
    enableChunked: true,
    responseType: 'arraybuffer',
  })

  task.onChunkReceived?.((r: any) => {
    const bytes = r?.data
    if (!bytes) return
    const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes)
    for (const ev of feedBytes(buf, u8)) handle(ev)
  })
  task.onClose?.(() => finish(closed ? undefined : new Error('流式连接中断（未收到结束帧）')))

  return {
    cancel: () => {
      closed = true
      try {
        task.abort?.()
      } catch {
        // 忽略
      }
    },
  }
}
