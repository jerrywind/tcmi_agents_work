import { useEffect, useState } from 'react'
import Taro from '@tarojs/taro'
import { View, Text, Input, Picker } from '@tarojs/components'
import { listMembers, newMemberId, removeMember, upsertMember } from '../../services/members'
import type { Member, PatientProfile } from '../../types'
import './index.scss'

const RELATIONS = ['本人', '父亲', '母亲', '配偶', '子女', '其他']
const GENDERS = ['男', '女', '未知']

const EMPTY_FORM = { name: '', relationIdx: 0, age: '', genderIdx: 2, note: '' }

/**
 * 家庭成员档案（**仅存本机**）。
 *
 * harness 是纯诊断服务，没有家庭/成员 API，因此这里用 Taro Storage 本地保存，
 * 作用是在「发起问诊」时预填体质档案。换设备或清缓存会丢失。
 */
export default function FamilyPage() {
  const [members, setMembers] = useState<Member[]>([])
  const [showAdd, setShowAdd] = useState(false)
  const [editing, setEditing] = useState<Member | null>(null)
  const [form, setForm] = useState({ ...EMPTY_FORM })

  useEffect(() => {
    setMembers(listMembers())
  }, [])

  const openAdd = () => {
    setEditing(null)
    setForm({ ...EMPTY_FORM })
    setShowAdd(true)
  }

  const openEdit = (m: Member) => {
    setEditing(m)
    setForm({
      name: m.name,
      relationIdx: Math.max(0, RELATIONS.indexOf(m.relation)),
      age: m.patient.age ? String(m.patient.age) : '',
      genderIdx: Math.max(0, GENDERS.indexOf(m.patient.gender)),
      note: m.note,
    })
    setShowAdd(true)
  }

  const save = () => {
    if (!form.name.trim()) {
      Taro.showToast({ title: '请填写称呼', icon: 'none' })
      return
    }
    const patient: PatientProfile = {
      age: parseInt(form.age, 10) || undefined,
      gender: GENDERS[form.genderIdx],
      region: editing?.patient.region,
      height_cm: editing?.patient.height_cm,
      weight_kg: editing?.patient.weight_kg,
    }
    const m: Member = {
      id: editing?.id || newMemberId(),
      name: form.name.trim(),
      relation: RELATIONS[form.relationIdx],
      patient,
      note: form.note.trim(),
    }
    upsertMember(m)
    setMembers(listMembers())
    setShowAdd(false)
    Taro.showToast({ title: '已保存', icon: 'success' })
  }

  const del = (m: Member) => {
    removeMember(m.id)
    setMembers(listMembers())
  }

  const startFor = (m: Member) => {
    Taro.navigateTo({ url: `/pages/index/index?mid=${m.id}` })
  }

  return (
    <View className='family-page'>
      <View className='fam-header'>
        <Text className='fam-title'>我的家庭</Text>
        <Text className='fam-sub'>{members.length} 位成员 · 仅存本机</Text>
      </View>

      <View className='member-list'>
        {members.map(m => (
          <View key={m.id} className='member-card'>
            <View className='member-top'>
              <View className='member-info'>
                <Text className='member-name'>{m.name}</Text>
                <Text className='member-rel'>{m.relation}</Text>
              </View>
              <View className='member-actions'>
                <View className='mini-btn' onClick={() => startFor(m)}>发起问诊</View>
                <View className='mini-btn ghost' onClick={() => openEdit(m)}>编辑</View>
                <View className='mini-btn danger' onClick={() => del(m)}>删除</View>
              </View>
            </View>
            <Text className='member-meta'>
              {m.patient.gender}
              {m.patient.age ? ` · ${m.patient.age}岁` : ''}
              {m.patient.height_cm ? ` · ${m.patient.height_cm}cm` : ''}
              {m.patient.weight_kg ? ` · ${m.patient.weight_kg}kg` : ''}
            </Text>
            {m.note ? <Text className='member-note'>备注：{m.note}</Text> : null}
          </View>
        ))}
      </View>

      {members.length === 0 && (
        <Text className='empty-tip'>还没有成员，添加后可一键预填问诊信息</Text>
      )}

      <View className='add-member' onClick={openAdd}>＋ 添加成员</View>

      {showAdd && (
        <View className='sheet-mask' onClick={() => setShowAdd(false)}>
          <View className='sheet' onClick={e => e.stopPropagation()}>
            <Text className='sheet-title'>{editing ? '编辑成员' : '添加成员'}</Text>
            <View className='form-row'>
              <Text className='form-label'>称呼</Text>
              <Input className='form-input' placeholder='如 父亲 / 女儿'
                value={form.name}
                onInput={e => setForm({ ...form, name: e.detail.value })} />
            </View>
            <Picker mode='selector' range={RELATIONS} value={form.relationIdx}
              onChange={e => setForm({ ...form, relationIdx: Number(e.detail.value) })}>
              <View className='form-row'>
                <Text className='form-label'>关系</Text>
                <Text className='form-input'>{RELATIONS[form.relationIdx]}</Text>
              </View>
            </Picker>
            <Picker mode='selector' range={GENDERS} value={form.genderIdx}
              onChange={e => setForm({ ...form, genderIdx: Number(e.detail.value) })}>
              <View className='form-row'>
                <Text className='form-label'>性别</Text>
                <Text className='form-input'>{GENDERS[form.genderIdx]}</Text>
              </View>
            </Picker>
            <View className='form-row'>
              <Text className='form-label'>年龄</Text>
              <Input className='form-input' type='number' placeholder='选填'
                value={form.age}
                onInput={e => setForm({ ...form, age: e.detail.value })} />
            </View>
            <View className='form-row'>
              <Text className='form-label'>备注</Text>
              <Input className='form-input' placeholder='过敏史/慢病等'
                value={form.note}
                onInput={e => setForm({ ...form, note: e.detail.value })} />
            </View>
            <View className='btn-primary' onClick={save}>
              {editing ? '保存修改' : '添加'}
            </View>
          </View>
        </View>
      )}
    </View>
  )
}
