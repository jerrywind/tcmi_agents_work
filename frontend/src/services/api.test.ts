import { describe, it, expect, vi, beforeEach } from 'vitest'
import Taro from '@tarojs/taro'
import {
  request, BASE_URL, createConsultation, getState,
  startConsultation, answerQuestion, uploadImage,
} from './api'

const mockedRequest = vi.mocked(Taro.request)
const mockedUpload = vi.mocked(Taro.uploadFile)

beforeEach(() => {
  vi.clearAllMocks()
})

describe('request', () => {
  it('returns data on 2xx', async () => {
    mockedRequest.mockResolvedValue({ statusCode: 200, data: { ok: true } } as any)
    const data = await request<{ ok: boolean }>('GET', '/ping')
    expect(data).toEqual({ ok: true })
    expect(mockedRequest).toHaveBeenCalledWith(expect.objectContaining({
      url: `${BASE_URL}/ping`,
      method: 'GET',
    }))
  })

  it('throws detail on >=400', async () => {
    mockedRequest.mockResolvedValue({ statusCode: 400, data: { detail: '参数错误' } } as any)
    await expect(request('POST', '/x')).rejects.toThrow('参数错误')
  })

  it('falls back to HTTP status message when no detail', async () => {
    mockedRequest.mockResolvedValue({ statusCode: 500, data: {} } as any)
    await expect(request('POST', '/x')).rejects.toThrow('HTTP 500')
  })

  it('falls back to HTTP status when body missing', async () => {
    mockedRequest.mockResolvedValue({ statusCode: 403 } as any)
    await expect(request('GET', '/x')).rejects.toThrow('HTTP 403')
  })

  it('throws network error when request rejects', async () => {
    mockedRequest.mockRejectedValue(new Error('fail'))
    await expect(request('GET', '/x')).rejects.toThrow('网络异常')
  })
})

describe('api wrappers', () => {
  it('createConsultation posts the right url', async () => {
    mockedRequest.mockResolvedValue({ statusCode: 200, data: { id: 'c1', status: 'created' } } as any)
    const r = await createConsultation(
      { region: '', height_cm: 170, weight_kg: 60, age: 30, gender: '男' },
      '主诉文本',
      {},
    )
    expect(r.id).toBe('c1')
    expect(mockedRequest).toHaveBeenCalledWith(expect.objectContaining({
      url: expect.stringContaining('/api/consultations'),
      method: 'POST',
    }))
  })

  it('getState maps to GET /consultations/:cid', async () => {
    mockedRequest.mockResolvedValue({ statusCode: 200, data: { id: 'c1', status: 'created' } } as any)
    await getState('c1')
    expect(mockedRequest).toHaveBeenCalledWith(expect.objectContaining({
      url: expect.stringContaining('/api/consultations/c1'),
      method: 'GET',
    }))
  })

  it('startConsultation posts start', async () => {
    mockedRequest.mockResolvedValue({ statusCode: 200, data: { status: 'waiting_answer' } } as any)
    await startConsultation('c1')
    expect(mockedRequest).toHaveBeenCalledWith(expect.objectContaining({
      url: expect.stringContaining('/api/consultations/c1/start'),
      method: 'POST',
    }))
  })

  it('answerQuestion posts answer', async () => {
    mockedRequest.mockResolvedValue({ statusCode: 200, data: { status: 'treatment_qa' } } as any)
    await answerQuestion('c1', 'q1', '可煎药')
    expect(mockedRequest).toHaveBeenCalledWith(expect.objectContaining({
      url: expect.stringContaining('/api/consultations/c1/answer'),
      method: 'POST',
      data: { question_id: 'q1', value: '可煎药' },
    }))
  })
})

describe('uploadImage', () => {
  it('resolves url on success', async () => {
    mockedUpload.mockImplementation((opts: any) => {
      opts.success({ statusCode: 200, data: JSON.stringify({ url: '/uploads/x.jpg' }) })
      return { onProgressUpdate: () => {}, abort: () => {} } as any
    })
    const r = await uploadImage('c1', 'tongue', '/tmp/x.jpg')
    expect(r).toEqual({ url: '/uploads/x.jpg' })
    expect(mockedUpload).toHaveBeenCalled()
  })

  it('rejects on failure', async () => {
    mockedUpload.mockImplementation((opts: any) => {
      opts.fail({ errMsg: 'network' })
      return {} as any
    })
    await expect(uploadImage('c1', 'tongue', '/tmp/x.jpg')).rejects.toThrow('图片上传失败')
  })

  it('rejects with body on >=400 in success', async () => {
    mockedUpload.mockImplementation((opts: any) => {
      opts.success({ statusCode: 500, data: 'server error' })
      return { onProgressUpdate: () => {}, abort: () => {} } as any
    })
    await expect(uploadImage('c1', 'tongue', '/tmp/x.jpg')).rejects.toThrow('server error')
  })

  it('resolves object data as-is when not a string', async () => {
    mockedUpload.mockImplementation((opts: any) => {
      opts.success({ statusCode: 200, data: { url: '/u.jpg' } })
      return { onProgressUpdate: () => {}, abort: () => {} } as any
    })
    await expect(uploadImage('c1', 'tongue', '/tmp/x.jpg')).resolves.toEqual({ url: '/u.jpg' })
  })

  it('resolves raw data when JSON parse fails', async () => {
    mockedUpload.mockImplementation((opts: any) => {
      opts.success({ statusCode: 200, data: 'not-json' })
      return { onProgressUpdate: () => {}, abort: () => {} } as any
    })
    await expect(uploadImage('c1', 'tongue', '/tmp/x.jpg')).resolves.toBe('not-json')
  })
})
