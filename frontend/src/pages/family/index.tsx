import { useEffect, useState } from 'react'
import Taro from '@tarojs/taro'
import { View, Text, Input, Textarea, Picker, ScrollView } from '@tarojs/components'
import { listMembers, newMemberId, removeMember, upsertMember } from '../../services/members'
import {
  EMPTY_PROFILE_FORM, GENDER_OPTIONS, buildProfile, describeProfile,
  toProfileForm, todayISO, validateProfileForm,
} from '../../utils/profile'
import BirthDatePicker from '../../components/BirthDatePicker'
import type { ProfileForm } from '../../utils/profile'
import type { Member } from '../../types'
import './index.scss'

const RELATIONS = ['本人', '父亲', '母亲', '配偶', '子女', '其他']
const BIRTH_DATE_START = '1900-01-01'

type MemberForm = ProfileForm & { relationIdx: number }

const EMPTY_FORM: MemberForm = { ...EMPTY_PROFILE_FORM, relationIdx: 0 }

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
  const [form, setForm] = useState<MemberForm>({ ...EMPTY_FORM })
  const [maxDate] = useState(todayISO())

  useEffect(() => {
    setMembers(listMembers())
  }, [])

  const patch = (p: Partial<MemberForm>) => setForm({ ...form, ...p })

  const openAdd = () => {
    setEditing(null)
    setForm({ ...EMPTY_FORM })
    setShowAdd(true)
  }

  const openEdit = (m: Member) => {
    setEditing(m)
    setForm({ ...toProfileForm(m.patient), name: m.name, relationIdx: Math.max(0, RELATIONS.indexOf(m.relation)) })
    setShowAdd(true)
  }

  const save = () => {
    if (!form.name.trim()) {
      Taro.showToast({ title: '请填写称呼', icon: 'none' })
      return
    }
    const err = validateProfileForm(form)
    if (err) {
      Taro.showToast({ title: err, icon: 'none' })
      return
    }
    const m: Member = {
      id: editing?.id || newMemberId(),
      name: form.name.trim(),
      relation: RELATIONS[form.relationIdx],
      patient: buildProfile(form),
    }
    upsertMember(m)
    setMembers(listMembers())
    setShowAdd(false)
    Taro.showToast({ title: '已保存', icon: 'success' })
  }

  /**
   * 删除成员：本地存储没有回收站，删掉就找不回来了，
   * 而卡片上「发起问诊 / 编辑 / 删除」三个按钮紧挨着，删除最不该被误触。
   */
  const del = (m: Member) => {
    Taro.showModal({
      title: '删除成员',
      content: `确定删除「${m.name}」的档案吗？其信息将从本机移除，且无法恢复。`,
      confirmText: '删除',
      confirmColor: '#c0392b',
      success: r => {
        if (!r.confirm) return
        removeMember(m.id)
        setMembers(listMembers())
        Taro.showToast({ title: '已删除', icon: 'success' })
      },
    })
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
            <Text className={`member-meta ${describeProfile(m.patient) ? '' : 'muted'}`}>
              {describeProfile(m.patient) || '档案不完整，点编辑补齐'}
            </Text>
          </View>
        ))}
      </View>

      {members.length === 0 && (
        <Text className='empty-tip'>还没有成员，添加后可一键预填问诊信息</Text>
      )}

      <View className='add-member' onClick={openAdd}>＋ 添加成员</View>

      {showAdd && (
        <View className='sheet-mask' onClick={() => setShowAdd(false)}>
          {/* 用 ScrollView 而非 View：弹层里是五项表单，矮屏上必然超出一屏，
              而 .sheet-mask 是 fixed 定位，内容溢出不会带动页面滚动 */}
          <ScrollView className='sheet' scrollY onClick={e => e.stopPropagation()}>
            <Text className='sheet-title'>{editing ? '编辑成员' : '添加成员'}</Text>
            <View className='form-row'>
              <Text className='form-label'>称呼</Text>
              <Input className='form-input' placeholder='如 父亲 / 女儿'
                value={form.name}
                onInput={e => patch({ name: e.detail.value })} />
            </View>
            <Picker mode='selector' range={RELATIONS} value={form.relationIdx}
              onChange={e => patch({ relationIdx: Number(e.detail.value) })}>
              <View className='form-row'>
                <Text className='form-label'>关系</Text>
                <Text className='form-input'>{RELATIONS[form.relationIdx]}</Text>
              </View>
            </Picker>
            {/* 与档案页同一个组件：两端各走各的实现，避免同一张表两处行为不一致 */}
            <BirthDatePicker
              value={form.birthDate}
              start={BIRTH_DATE_START}
              end={maxDate}
              onChange={v => patch({ birthDate: v })}
            />
            <View className='form-row'>
              <Text className='form-label'>性别</Text>
              <View className='gender-group'>
                {GENDER_OPTIONS.map(g => (
                  <View key={g} className={`gender-chip ${form.gender === g ? 'active' : ''}`}
                    onClick={() => patch({ gender: g })}>
                    {g}
                  </View>
                ))}
              </View>
            </View>
            <View className='form-row'>
              <Text className='form-label'>常住地</Text>
              <Input className='form-input' placeholder='如：广州'
                value={form.region}
                onInput={e => patch({ region: e.detail.value })} />
            </View>
            <View className='form-row col'>
              <Text className='form-label'>既往病史（选填）</Text>
              <Textarea className='history-input' maxlength={500}
                placeholder='慢病、过敏史、手术史、长期用药等'
                value={form.history}
                onInput={e => patch({ history: e.detail.value })} />
            </View>
            <View className='btn-primary' onClick={save}>
              {editing ? '保存修改' : '添加'}
            </View>
          </ScrollView>
        </View>
      )}
    </View>
  )
}
