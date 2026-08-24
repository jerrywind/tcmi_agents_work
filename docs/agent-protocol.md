# Sub-Agent 可替换协议规范 v1

## 设计目标

诊断流程中的每个子项（望/闻/问/切/辨证/安全）都可能需要独立换模型或换实现
（规则引擎 ↔ LLM ↔ 远程服务）。本协议保证：**切换实现 = 改一行配置，编排器与其他
Sub-Agent 零改动**。

## 1. 能力标识（Capability）

每个诊断子项是一个 capability，采用命名空间字符串：

| Capability | 子项 | 产出 |
|---|---|---|
| `diagnosis.inspection` | 望诊 | 舌象/面色/患处证据 |
| `diagnosis.listening` | 闻诊 | 声/味/文本线索证据 |
| `diagnosis.inquiry` | 问诊 | 下一个最优问题 |
| `diagnosis.palpation` | 切诊 | 脉率证据（低置信度） |
| `diagnosis.differentiation` | 辨证 | 候选证候 + 置信度 + 证据链 |
| `diagnosis.safety` | 安全 | 红旗告警 |
| `treatment.plan` | 诊疗方案 | 多模态方案 `plans`（开方/针灸/西医检查/调护）+ 个性化追问 `question` |

> `treatment.plan` 在辨证完成后触发：可先就"煎药便利性/是否接受外治/是否愿做西医检查/
> 是否孕期备孕"追问 1~2 条，再产出以"更快、更彻底痊愈"为目标的综合方案，不限于开中药，
> 还包含针灸推拿、外治法、西医检查（明确诊断/排除器质病变）、生活调护等。

## 2. 统一信封（Envelope）

所有调用使用强类型信封（Pydantic，可 JSON 序列化 → 天然支持远程化）：

```
AgentRequest {
  request_id, capability, session_id, round,
  payload: dict          # capability 特定输入
  evidences: [Evidence]  # 证据池只读快照
  hypotheses: [Hypothesis]
  asked_keys: [str]
  model: str             # 路由指定的"逻辑模型名"
  options: dict          # 路由透传参数
}

AgentResponse {
  request_id, capability,
  status: ok | error | skip,
  evidences / hypotheses / question / alerts / plans,   # 按 capability 产出其一或多个
  notes, error,
  meta: { impl, model, latency_ms }             # 可观测性
}
```

关键约定：
- **无状态**：Sub-Agent 不持有会话状态，一切经信封进出 → 可随时替换/并行/分布式。
- **只增不改**：Sub-Agent 只产出新证据/新假设，证据合并策略由编排器统一执行。
- **统一容错**：基类 `SubAgent.run()` 包装 `handle()`，异常降级为 `status=error`；
  编排器对 error/skip 一视同仁地继续流程（安全 agent 除外）。

## 3. 注册与路由

```python
@register
class InspectionVisionAgent(SubAgent):
    capability = Capability.INSPECTION
    impl_name = "llm_vision"
```

`routing.yaml` 决定运行时用哪个实现、哪个模型：

```yaml
routing:
  diagnosis.inspection:
    impl: llm_vision        # 切换实现：rule -> llm_vision
    model: vision-default   # 逻辑模型名
llm:
  models:
    vision-default: gpt-4o  # 逻辑名 -> 实际模型，换模型只改这里
```

解析链：`resolve(capability)` → 查路由 → 注册表取实现 → 实现缺失自动降级 `rule`。

## 4. 模型抽象

Sub-Agent 的 LLM 实现只依赖 `LLMProvider.chat()`（openai 兼容协议）；
`model` 字段是逻辑名，由 `llm.models` 映射到实际模型 id。未配置 API Key 时
Provider 自动降级为 Mock，LLM 实现内部再回退到规则结果，保证全链路可运行。

## 5. 扩展方式

- **新增实现**：继承 `SubAgent`、声明 `capability + impl_name`、加 `@register`，
  改 routing.yaml 即上线；可通过 `GET /api/system/agents` 查看当前路由与可用实现。
- **远程 Sub-Agent**：实现一个 `HttpProxyAgent(SubAgent)`，把信封 POST 给远程服务
  （信封本身就是 JSON 契约），对编排器完全透明。
- **灰度/AB**：在 resolve() 层按 session_id 哈希分流不同 impl（预留扩展点）。

## 6. 可观测性

- 每次调用的 `{capability, impl, status, latency_ms, error}` 记入会话 `trace`，
  `GET /api/consultations/{id}/trace` 可查每一轮由哪个实现/模型处理。
