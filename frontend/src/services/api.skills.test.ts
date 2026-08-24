import { describe, it, expect, vi, beforeEach } from 'vitest'
import Taro from '@tarojs/taro'
import { getSkills, loadSkill, unloadSkill } from './api'

const mockedRequest = vi.mocked(Taro.request)

describe('skills API', () => {
  beforeEach(() => {
    mockedRequest.mockReset()
  })

  it('getSkills fetches the skills list', async () => {
    mockedRequest.mockResolvedValue({
      statusCode: 200,
      data: { skills_dir: '/app/skills', skills: [{ name: 'tcm-kb', version: '0.1.0', description: 'd', tools: [] }], tools: [] },
    } as any)
    const list = await getSkills()
    expect(mockedRequest).toHaveBeenCalledWith(expect.objectContaining({
      url: 'http://127.0.0.1:8000/api/skills',
      method: 'GET',
    }))
    expect(list.skills[0].name).toBe('tcm-kb')
  })

  it('loadSkill posts name/path', async () => {
    mockedRequest.mockResolvedValue({
      statusCode: 200,
      data: { name: 'tcm-kb', version: '0.1.0', description: '', tools: [] },
    } as any)
    const skill = await loadSkill('tcm-kb')
    expect(mockedRequest).toHaveBeenCalledWith(expect.objectContaining({
      url: 'http://127.0.0.1:8000/api/skills/load',
      method: 'POST',
      data: { name: 'tcm-kb', path: undefined },
    }))
    expect(skill.name).toBe('tcm-kb')
  })

  it('unloadSkill posts name', async () => {
    mockedRequest.mockResolvedValue({
      statusCode: 200,
      data: { ok: true, unloaded: 'tcm-kb' },
    } as any)
    const res = await unloadSkill('tcm-kb')
    expect(mockedRequest).toHaveBeenCalledWith(expect.objectContaining({
      url: 'http://127.0.0.1:8000/api/skills/unload',
      method: 'POST',
      data: { name: 'tcm-kb' },
    }))
    expect(res.ok).toBe(true)
  })

  it('throws on >=400', async () => {
    mockedRequest.mockResolvedValue({ statusCode: 400, data: { detail: '未知技能' } } as any)
    await expect(loadSkill('nope')).rejects.toThrow('未知技能')
  })
})
