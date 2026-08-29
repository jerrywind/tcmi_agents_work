import Taro from '@tarojs/taro'
import type { Member } from '../types'

/**
 * 家庭成员档案的**本地**存储。
 *
 * harness 是纯诊断服务，**没有**家庭 / 成员 API。
 * 因此成员档案只存在前端（Taro Storage），用途是发起问诊时预填体质档案。
 * 请勿把它当成后端实体——换设备或清缓存即丢失。
 */
const STORAGE_KEY = 'tcm_members_v1'

export function listMembers(): Member[] {
  try {
    const raw = Taro.getStorageSync(STORAGE_KEY)
    if (!raw) return []
    const arr = typeof raw === 'string' ? JSON.parse(raw) : raw
    return Array.isArray(arr) ? arr : []
  } catch {
    return []
  }
}

function persist(list: Member[]): void {
  try {
    Taro.setStorageSync(STORAGE_KEY, JSON.stringify(list))
  } catch {
    /* 存储不可用时静默降级为「仅本次会话有效」 */
  }
}

export function getMember(id: string): Member | null {
  return listMembers().find(m => m.id === id) || null
}

export function upsertMember(m: Member): void {
  const list = listMembers()
  const i = list.findIndex(x => x.id === m.id)
  if (i >= 0) list[i] = m
  else list.push(m)
  persist(list)
}

export function removeMember(id: string): void {
  persist(listMembers().filter(m => m.id !== id))
}

export function newMemberId(): string {
  return `m_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 8)}`
}
