import Taro from '@tarojs/taro'
import type { CareTodo, ConsultState, Family, FamilyConsultSummary, FollowUp, LabResult, Member, PatientForm, RevisitCompare, RevisitImage, Skill, SkillsList } from '../types'

// H5 走 devServer 代理；小程序/RN 直连后端地址（可用 VITE_API_BASE 覆盖，便于契约测试）
export const BASE_URL =
  process.env.TARO_ENV === 'h5'
    ? ''
    : process.env.VITE_API_BASE || 'http://127.0.0.1:8000'

export async function request<T>(method: 'GET' | 'POST' | 'PATCH', url: string, data?: any): Promise<T> {
  let res
  try {
    res = await Taro.request({
      url: `${BASE_URL}${url}`,
      method,
      data,
      header: { 'Content-Type': 'application/json' },
      timeout: 120000,
    })
  } catch {
    throw new Error('网络异常')
  }
  if (res.statusCode >= 400) {
    const detail = (res.data && (res.data as any).detail) || `HTTP ${res.statusCode}`
    throw new Error(String(detail))
  }
  return res.data as T
}

export function createConsultation(patient: PatientForm, complaint: string,
  selfReport: Record<string, any>, familyId: string = '', memberId: string = ''): Promise<ConsultState> {
  return request('POST', '/api/consultations',
    { patient, complaint, self_report: selfReport, family_id: familyId, member_id: memberId })
}

export function uploadImage(cid: string, type: 'tongue' | 'face' | 'lesion' | 'palm_left' | 'palm_right',
  filePath: string): Promise<any> {
  return new Promise((resolve, reject) => {
    Taro.uploadFile({
      url: `${BASE_URL}/api/consultations/${cid}/images`,
      filePath,
      name: 'file',
      formData: { type },
      success: (r) => {
        if (r.statusCode >= 400) {
          reject(new Error(String(r.data)))
          return
        }
        try {
          resolve(typeof r.data === 'string' ? JSON.parse(r.data) : r.data)
        } catch {
          resolve(r.data)
        }
      },
      fail: () => reject(new Error('图片上传失败')),
    })
  })
}

export function startConsultation(cid: string): Promise<ConsultState> {
  return request('POST', `/api/consultations/${cid}/start`)
}

export function answerQuestion(cid: string, questionId: string, value: string,
  text: string = ''): Promise<ConsultState> {
  return request('POST', `/api/consultations/${cid}/answer`,
    { question_id: questionId, value, text })
}

export function getState(cid: string): Promise<ConsultState> {
  return request('GET', `/api/consultations/${cid}`)
}

export interface StreamSeg {
  seq: number
  role: 'agent' | 'user' | 'system'
  type: 'text' | 'question' | 'report' | 'alert'
  content: string
  done: boolean
}

export function getStream(cid: string, after: number): Promise<{
  task: 'running' | 'done' | 'error'
  error: string | null
  segs: StreamSeg[]
}> {
  return request('GET', `/api/consultations/${cid}/stream?after=${after}`)
}

export function getCare(cid: string): Promise<CareTodo[]> {
  return request('GET', `/api/consultations/${cid}/care`)
}

export function checkCare(cid: string, todoId: string): Promise<CareTodo> {
  return request('POST', `/api/consultations/${cid}/care/check`, { todo_id: todoId })
}

export function getFollowups(cid: string): Promise<FollowUp[]> {
  return request('GET', `/api/consultations/${cid}/followups`)
}

export function postFollowupFeedback(cid: string, fid: string, feedback: string): Promise<any> {
  return request('POST', `/api/consultations/${cid}/followup/${fid}/feedback`, { feedback })
}

export function postRevisit(cid: string, path: string, kind: string, selfReport: Record<string, string> = {}): Promise<RevisitImage> {
  return request('POST', `/api/consultations/${cid}/revisit`, { path, kind, self_report: selfReport })
}

export function getRevisitCompare(cid: string): Promise<RevisitCompare> {
  return request('GET', `/api/consultations/${cid}/revisit/compare`)
}

export function postLab(cid: string, text: string): Promise<LabResult> {
  return request('POST', `/api/consultations/${cid}/lab`, { text })
}

// ---------------- 家庭 / 成员 ----------------
export function createFamily(name: string, owner: string = ''): Promise<Family> {
  return request('POST', '/api/families', { name, owner })
}

export function listFamilies(): Promise<Family[]> {
  return request('GET', '/api/families')
}

export function getFamily(fid: string): Promise<Family> {
  return request('GET', `/api/families/${fid}`)
}

export function addMember(fid: string, name: string, relation: string,
  patient: PatientForm = {} as PatientForm, note: string = ''): Promise<Member> {
  return request('POST', `/api/families/${fid}/members`,
    { name, relation, patient, note })
}

export function updateMember(fid: string, mid: string, name: string, relation: string,
  patient: PatientForm = {} as PatientForm, note: string = ''): Promise<Member> {
  return request('PATCH', `/api/families/${fid}/members/${mid}`,
    { name, relation, patient, note })
}

export function familyConsultations(fid: string, memberId: string = ''): Promise<FamilyConsultSummary[]> {
  const q = memberId ? `?member_id=${encodeURIComponent(memberId)}` : ''
  return request('GET', `/api/families/${fid}/consultations${q}`)
}

// ---------------- PPG 脉象 ----------------
export function postPpg(cid: string, opts: {
  samples?: number[]; fs?: number; simulate?: boolean; profile?: string; rate_bpm?: number
} = {}): Promise<ConsultState> {
  return request('POST', `/api/consultations/${cid}/ppg`, opts)
}

// ---------------- SKILL 管理（LLM 可调用工具集） ----------------
export function getSkills(): Promise<SkillsList> {
  return request('GET', '/api/skills')
}

export function loadSkill(name?: string, path?: string): Promise<Skill> {
  return request('POST', '/api/skills/load', { name, path })
}

export function unloadSkill(name: string): Promise<{ ok: boolean; unloaded: string }> {
  return request('POST', '/api/skills/unload', { name })
}
