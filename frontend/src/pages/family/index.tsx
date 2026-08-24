import { useEffect, useState } from 'react'
import Taro from '@tarojs/taro'
import { View, Text, Input, Picker } from '@tarojs/components'
import {
  addMember, createFamily, familyConsultations, getFamily, listFamilies, updateMember
} from '../../services/api'
import type { Family, FamilyConsultSummary, Member } from '../../types'
import './index.scss'

const RELATIONS = ['本人', '父亲', '母亲', '配偶', '子女', '其他']

export default function FamilyPage() {
  const [family, setFamily] = useState<Family | null>(null)
  const [loading, setLoading] = useState(true)
  const [consults, setConsults] = useState<FamilyConsultSummary[]>([])
  const [showAdd, setShowAdd] = useState(false)
  const [editing, setEditing] = useState<Member | null>(null)
  const [name, setName] = useState('')
  const [relationIdx, setRelationIdx] = useState(0)
  const [age, setAge] = useState('')
  const [genderIdx, setGenderIdx] = useState(2)
  const [note, setNote] = useState('')
  const [saving, setSaving] = useState(false)

  const GENDERS = ['男', '女', '未知']

  const refresh = async (fid: string) => {
    const f = await getFamily(fid)
    setFamily(f)
    const cs = await familyConsultations(fid)
    setConsults(cs)
  }

  useEffect(() => {
    ;(async () => {
      try {
        const fs = await listFamilies()
        if (fs.length) {
          await refresh(fs[0].id)
        }
      } catch (e: any) {
        Taro.showToast({ title: e?.message || '加载失败', icon: 'none' })
      } finally {
        setLoading(false)
      }
    })()
  }, [])

  const doCreateFamily = async () => {
    setSaving(true)
    try {
      const f = await createFamily('我的家庭')
      setFamily(f)
      setConsults([])
    } catch (e: any) {
      Taro.showToast({ title: e?.message || '创建失败', icon: 'none' })
    } finally {
      setSaving(false)
    }
  }

  const openAdd = () => {
    setEditing(null)
    setName(''); setRelationIdx(0); setAge(''); setGenderIdx(2); setNote('')
    setShowAdd(true)
  }

  const openEdit = (m: Member) => {
    setEditing(m)
    setName(m.name)
    setRelationIdx(Math.max(0, RELATIONS.indexOf(m.relation)))
    setAge(String(m.patient.age || ''))
    setGenderIdx(Math.max(0, GENDERS.indexOf(m.patient.gender)))
    setNote(m.note)
    setShowAdd(true)
  }

  const saveMember = async () => {
    if (!family || !name.trim() || saving) return
    setSaving(true)
    Taro.showLoading({ title: '保存中...' })
    try {
      const patient = {
        region: editing?.patient.region || '',
        height_cm: editing?.patient.height_cm || 0,
        weight_kg: editing?.patient.weight_kg || 0,
        age: parseInt(age) || 0,
        gender: GENDERS[genderIdx]
      }
      if (editing) {
        await updateMember(family.id, editing.id, name.trim(), RELATIONS[relationIdx], patient, note.trim())
      } else {
        await addMember(family.id, name.trim(), RELATIONS[relationIdx], patient, note.trim())
      }
      await refresh(family.id)
      setShowAdd(false)
    } catch (e: any) {
      Taro.showToast({ title: e?.message || '保存失败', icon: 'none' })
    } finally {
      setSaving(false)
      Taro.hideLoading()
    }
  }

  const startFor = (m: Member) => {
    if (!family) return
    Taro.navigateTo({ url: `/pages/index/index?fid=${family.id}&mid=${m.id}` })
  }

  const historyOf = (mid: string) =>
    consults.filter(c => c.member_id === mid)

  if (loading) return <View className='family-page' />

  if (!family) {
    return (
      <View className='family-page empty'>
        <Text className='empty-tip'>还没有家庭档案</Text>
        <View className={`btn-primary ${saving ? 'disabled' : ''}`} onClick={doCreateFamily}>
          创建我的家庭
        </View>
      </View>
    )
  }

  return (
    <View className='family-page'>
      <View className='fam-header'>
        <Text className='fam-title'>{family.name}</Text>
        <Text className='fam-sub'>{family.members.length} 位成员</Text>
      </View>

      <View className='member-list'>
        {family.members.map(m => {
          const hs = historyOf(m.id)
          return (
            <View key={m.id} className='member-card'>
              <View className='member-top'>
                <View className='member-info'>
                  <Text className='member-name'>{m.name}</Text>
                  <Text className='member-rel'>{m.relation}</Text>
                </View>
                <View className='member-actions'>
                  <View className='mini-btn' onClick={() => startFor(m)}>发起问诊</View>
                  <View className='mini-btn ghost' onClick={() => openEdit(m)}>编辑</View>
                </View>
              </View>
              {m.patient.age > 0 || m.patient.gender !== '未知' ? (
                <Text className='member-meta'>
                  {m.patient.gender}{m.patient.age ? ` · ${m.patient.age}岁` : ''}
                  {m.patient.height_cm ? ` · ${m.patient.height_cm}cm` : ''}
                  {m.patient.weight_kg ? ` · ${m.patient.weight_kg}kg` : ''}
                </Text>
              ) : <Text className='member-meta muted'>未填体质档案</Text>}
              {m.note ? <Text className='member-note'>备注：{m.note}</Text> : null}
              {hs.length > 0 ? (
                <View className='member-history'>
                  {hs.slice(0, 3).map(h => (
                    <View key={h.id} className='hist-item'
                      onClick={() => Taro.navigateTo({ url: `/pages/report/index?id=${h.id}` })}>
                      <Text className='hist-complaint'>{h.complaint || '问诊'}</Text>
                      <Text className='hist-syn'>
                        {h.syndromes.length ? h.syndromes.join('、') : h.status}
                      </Text>
                    </View>
                  ))}
                </View>
              ) : <Text className='member-note muted'>暂无问诊记录</Text>}
            </View>
          )
        })}
      </View>

      <View className='add-member' onClick={openAdd}>＋ 添加成员</View>

      {showAdd && (
        <View className='sheet-mask' onClick={() => setShowAdd(false)}>
          <View className='sheet' onClick={e => e.stopPropagation()}>
            <Text className='sheet-title'>{editing ? '编辑成员' : '添加家庭成员'}</Text>
            <View className='form-row'>
              <Text className='form-label'>称呼</Text>
              <Input className='form-input' placeholder='如 父亲 / 女儿'
                value={name} onInput={e => setName(e.detail.value)} />
            </View>
            <Picker mode='selector' range={RELATIONS} value={relationIdx}
              onChange={e => setRelationIdx(Number(e.detail.value))}>
              <View className='form-row'>
                <Text className='form-label'>关系</Text>
                <Text className='form-input'>{RELATIONS[relationIdx]}</Text>
              </View>
            </Picker>
            <Picker mode='selector' range={GENDERS} value={genderIdx}
              onChange={e => setGenderIdx(Number(e.detail.value))}>
              <View className='form-row'>
                <Text className='form-label'>性别</Text>
                <Text className='form-input'>{GENDERS[genderIdx]}</Text>
              </View>
            </Picker>
            <View className='form-row'>
              <Text className='form-label'>年龄</Text>
              <Input className='form-input' type='number' placeholder='选填'
                value={age} onInput={e => setAge(e.detail.value)} />
            </View>
            <View className='form-row'>
              <Text className='form-label'>备注</Text>
              <Input className='form-input' placeholder='过敏史/慢病等'
                value={note} onInput={e => setNote(e.detail.value)} />
            </View>
            <View className={`btn-primary ${saving ? 'disabled' : ''}`} onClick={saveMember}>
              {editing ? '保存修改' : '添加'}
            </View>
          </View>
        </View>
      )}
    </View>
  )
}
