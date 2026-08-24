// @vitest-environment node
/**
 * 前端 ↔ 后端「契约测试」：用真实 Node fetch 替身替换 Taro 运行时，
 * 让 src/services/api.ts 的客户端直连真实后端服务，验证请求/响应契约一致。
 *
 * 设计：
 * - 使用 node 环境（而非默认 jsdom），保证 fetch / FormData / Blob 为真实 Node 实现，
 *   避免 jsdom 下的 FormData 与 Node fetch 不兼容导致 multipart 体为空。
 * - 通过顶层 await 探测 BASE 处健康端点；不可达则整个 describe 自动 skip，
 *   因此 CI（无后端）下 npm run test 仍全绿，开发者本地起后端后自动执行。
 * - 不改 api.ts 业务，只把 Taro.request / Taro.uploadFile 重定向到真实 HTTP，
 *   从而端到端校验客户端封装与后端 API 的契约（字段、状态码、错误形态）。
 */
import { describe, it, expect } from 'vitest'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import Taro from '@tarojs/taro'
import * as api from './api'

const BASE = process.env.VITE_API_BASE || 'http://127.0.0.1:8000'

async function ping(): Promise<boolean> {
  try {
    const r = await fetch(`${BASE}/api/health`)
    return r.ok
  } catch {
    return false
  }
}

// 用真实 Node fetch 替身替换 Taro 运行时，使 api.ts 直连后端
function installRealTransport() {
  ;(Taro as any).request = async (opts: any) => {
    const res = await fetch(opts.url, {
      method: opts.method,
      headers: { 'Content-Type': 'application/json', ...(opts.header || {}) },
      body: opts.data !== undefined ? JSON.stringify(opts.data) : undefined,
    })
    const text = await res.text()
    let data: any = text
    try {
      data = text ? JSON.parse(text) : null
    } catch {
      /* 非 JSON 原文透传 */
    }
    return { statusCode: res.status, data, header: {} }
  }
  ;(Taro as any).uploadFile = (opts: any) => {
    const buf = fs.readFileSync(opts.filePath)
    const fd = new FormData()
    fd.append(opts.name, new Blob([buf], { type: 'image/jpeg' }), path.basename(opts.filePath))
    for (const [k, v] of Object.entries(opts.formData || {})) fd.append(k, String(v))
    fetch(opts.url, { method: 'POST', body: fd })
      .then(async (res) => {
        const t = await res.text()
        opts.success?.({ statusCode: res.status, data: t })
      })
      .catch(() => opts.fail?.())
    return {} as any
  }
}

const online = await ping()
if (online) installRealTransport()

describe.skipIf(!online)(
  `前端↔后端契约测试（直连真实服务 @ ${BASE}）`,
  () => {
    let cid = ''

    it('createConsultation 返回 created 状态与会话 id', async () => {
      const st = await api.createConsultation(
        { gender: '男' }, '口苦口臭大便粘马桶身体困重', {},
      )
      expect(typeof st.id).toBe('string')
      expect(st.id.length).toBeGreaterThan(0)
      expect(st.status).toBe('created')
      cid = st.id
    })

    it('startConsultation 进入诊断并返回首个问题或转诊', async () => {
      const st = await api.startConsultation(cid)
      expect([
        'waiting_answer', 'referred', 'running', 'planning', 'finished',
      ]).toContain(st.status)
    })

    it('answerQuestion 推进对话状态', async () => {
      const st = await api.getState(cid)
      if (st.status === 'waiting_answer' && st.question) {
        const next = await api.answerQuestion(
          cid, st.question.id, st.question.options?.[0]?.value ?? '无', '',
        )
        expect(typeof next.status).toBe('string')
      }
    })

    it('getState 返回与会话一致的会话状态', async () => {
      const st = await api.getState(cid)
      expect(st.id).toBe(cid)
    })

    it('getSkills 暴露内置技能与工具清单', async () => {
      const list = await api.getSkills()
      expect(Array.isArray(list.skills)).toBe(true)
      expect(list.skills.some((s: any) => s.name === 'tcm-kb')).toBe(true)
      expect(Array.isArray(list.tools)).toBe(true)
    })

    it('loadSkill 按名称装载并返回技能清单', async () => {
      const skill = await api.loadSkill('tcm-kb')
      expect(skill.name).toBe('tcm-kb')
    })

    it('unloadSkill 卸载不存在的技能按契约抛错（后端 404 -> Error）', async () => {
      await expect(api.unloadSkill('__not_exist__')).rejects.toThrow()
    })

    it('uploadImage 上传图片并返回 /uploads 静态地址', async () => {
      // 上传要求会话处于 created 态，故使用独立新建的会话
      const fresh = await api.createConsultation({ gender: '男' }, '口苦口臭', {})
      const tmp = path.join(os.tmpdir(), `tongue_${Date.now()}.jpg`)
      fs.writeFileSync(tmp, Buffer.from([0xff, 0xd8, 0xff, 0xe0, 0x4a, 0x46, 0x49, 0x46]))
      const r = await api.uploadImage(fresh.id, 'tongue', tmp)
      expect(r.url.startsWith('/uploads/')).toBe(true)
      fs.unlinkSync(tmp)
    })
  },
)
