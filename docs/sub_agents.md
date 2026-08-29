# Sub-Agent 设计：职责、实现与 Prompt

本文档描述 7 个 Sub-Agent 各自的职责边界、**规则层与 LLM 层的组合方式**，
以及 prompt 与知识数据的维护位置。

- 实现：`server/harness/src/agents/*.rs`
- System Prompt：`server/harness/resources/prompts.yaml`（**中文，可改**）
- 规则数据：`server/harness/resources/*.yaml`
- 流程顺序：`server/harness/resources/routing.yaml`
- 协议：[`agent-protocol.md`](./agent-protocol.md)；技能：[`skills.md`](./skills.md)

> **共同实现模式**：每个 Agent 都是「**可选规则前置/后置 + 一次 LLM 调用**」，
> 统一走 `ctx.caller().chat_with_tools(...)`（`agents/base.rs` 的 `LlmCaller`），
> 固定 `temperature: 0.3`、`stream: false`。
>
> `LlmCaller` 集中处理三件事：
> - **多轮工具调用**：最多 `max_tool_rounds` 轮（默认 3），达上限后转一次纯汇总调用；
> - **失败重试**：超时/连接失败/5xx/429 重试 `llm_max_retries` 次（默认 2），指数退避；
> - **埋点**：耗时、token、工具调用名、错误写入步骤 `TraceHandle`，由编排器汇总为 `trace[]`。
>
> **没有 rule/mock 实现**：LLM 不可用时该 Agent 返回 `Err`。但编排器**不会因此中断整个
> `/chat`**：失败步骤记入 `failures[]` 并置 `partial: true`，已完成的步骤照常返回。

---

## 总览

| capability | 角色 | 实现类 | 规则层 | 可见技能 |
|---|---|---|---|---|
| `inspection` | 望诊 | `InspectionAgent` | 后置：关键词证据叠加 | `tcm-vision` + 3 全局 |
| `listening` | 闻诊 | `ListeningAgent` | 后置：关键词证据叠加 | `tcm-auscultation` + 3 全局 |
| `inquiry` | 问诊 | `InquiryAgent` | 后置：`questions.yaml` 未覆盖问题去重取 top6 | `tcm-inquiry` + 3 全局 |
| `palpation` | 切诊 | `PalpationAgent` | 后置：`parse_ppg` 体检数值解析 | `tcm-palpation` + 3 全局 |
| `differentiation` | 辨证 | `DifferentiationAgent` | **前置**：证候库结构化打分（主证/兼证 + 置信度 + 支持/矛盾证据）+ 传变提示；并产出 `structured` | `tcm-reference` + 3 全局 |
| `safety` | 安全门 | `SafetyAgent` | **前置**（强制）：红旗扫描 + 用药禁忌 | `tcm-safety` + 3 全局 |
| `treatment` | 治疗 | `TreatmentAgent` | **前置**：方剂/调护检索 + 用药安全 | `tcm-formula`、`tcm-care` + 3 全局 |

「3 全局」= `tcm-kb`、`tcm-diet`、`tcm-rag`；若 `config.yaml` 配了 `mcp_clients`，
每个 Agent 还会额外看到全部 `mcp__*` 工具。见 [`skills.md`](./skills.md) 第 2.1 节。

> **工具调用已在链路中生效**：7 个 Agent 全部走 `chat_with_tools`，上表「可见技能」
> 即推理中模型真正能调用的工具，调用轨迹见 `/chat` 响应的 `trace[].tool_calls`。
> 另：`safety` 命中 `high`/`critical` 红旗时，编排器会**跳过其后的步骤**（默认即
> `treatment`），见下文安全门一节与 [`usage.md`](./usage.md) 2.3。

---

## 各 Sub-Agent 详解

### 望诊 `inspection`
- **职责**：解读舌象/面象/患处的文字描述，输出客观观察。
- **实现**：`chat_with_tools(system=prompts.inspection, cap=inspection)` → 取最后一条 user 消息跑
  `match_keywords()`，命中则追加 `[望诊证据] ...`。
- **图片**：harness 不落盘、不上传。多模态图片需由调用方以 base64/URL 形式随
  `messages` 传入，由模型端点自行理解（默认 `google/gemma-4-12b-qat` 原生多模态）。

### 闻诊 `listening`
- **职责**：从口语化自述中抽取声、嗅、咳、息线索。
- **实现**：`chat_with_tools(system=prompts.listening, cap=listening)` → `match_keywords()` 追加 `[闻诊证据]`。

### 问诊 `inquiry`
- **职责**：提出下一个最有助于鉴别的问题。
- **实现**：
  1. 规则层：把 `questions.yaml` 中 `evidence_keys` 未被对话文本覆盖的题目过滤出来，
     按 `priority` 升序取前 6 条，拼成 `【建议追问】`；
  2. LLM 层：`chat_with_tools(system=prompts.inquiry, cap=inquiry)` 生成自然语言追问；
  3. 输出 = LLM 部分 + `【建议追问】`。

### 切诊 `palpation`
- **职责**：脉象描述 + 体检数值结构化。
- **实现**：`chat_with_tools(system=prompts.palpation, cap=palpation)` → 对最后一条 user 消息跑
  `knowledge::parse_ppg()`，解析出收缩压/舒张压/体温/心率/血糖等，追加 `[体检数据解析] {...}`。

### 辨证 `differentiation`
- **职责**：综合四诊给出候选证候（主证 + 兼证，各带置信度与证据链）。
- **实现**：
  1. **前置规则（结构化，`assess()` 纯函数，T4.1/T4.2）**：
     - 对 `syndromes.yaml` 每个证候统计证据量：症状命中 +1，舌象/脉象命中 +1
       （短语按「，」拆段匹配），命中的关键词证据 +0.5，每条矛盾证据 −0.5；
     - 置信度 = `min(1, 证据量 / 5)`，保留两位小数；
     - **矛盾证据**：命中表现的**相反表现**若出现在语料中即计入，
       相反表现表在 `resources/contradictions.yaml`（中医可维护）；
     - 主证 = 证据量最高者（同分保持证候库顺序）；
       **兼证** = 其余候选中置信度 ≥ 0.2 且证据量 ≥ 主证 60% 者，按证据量降序；
     - 传变提示：按主证 slug 查 `transformations.yaml` 的 `from`。
  2. **LLM 层**：`chat_with_tools(system=prompts.differentiation + 初筛摘要, cap=differentiation)`
     ——把结构化初筛结论附进系统提示，让模型有据可依而非凭空起证名；
  3. **输出**：正文 = 结构化小节（`【结构化辨证】` 主证/兼证/置信度/支持/矛盾 + 传变提示）
     + LLM 结论；同时经 `structured()` 把同一份结构化结论随响应返回
     （`/chat` 的 `structured.differentiation`、`POST /agents` 的 `structured`）。
- **契约要点**：结构化部分是**确定性**的（同一语料必得同一结论、不依赖 LLM），
  因此可写回归测试，前端也可直接按字段渲染。

### 安全门 `safety`（**唯一强制规则前置**）
- **职责**：红旗识别 + 用药安全校验。
- **实现**：
  1. **红旗扫描（永远执行）**：遍历 `safety.yaml` 的 `red_flags`，命中关键词即产出
     `[{severity}] {advice}`；
  2. **用药安全（条件执行）**：仅当 `payload.herbs` 为字符串数组时，调用
     `knowledge::check_herb_safety(herbs, payload.pregnant)`，
     校验十八反（15 组）/十九畏（9 组）/妊娠禁忌（16 味）；
  3. 两者皆空 → 直接返回「未触发红色警戒」，**不调用 LLM**；
  4. 否则调用 `chat_with_tools(system=prompts.safety, cap=safety)` 追加解释（prompt 为空则跳过）；
  5. 末尾追加结构化回执：`{"red_flags":[{slug,label,severity,advice}], "herb_safety":[...], "blocked": bool, "severity": string}`。
- **中断契约**：`severity` 为 `high`/`critical` 时回执里 `blocked: true`；编排器用**同一套判定**
  （`agents::blocking_red_flag`）跳过其后的步骤，并在 `/chat` 响应置 `blocked`/
  `block_reason`/`skipped[]`。`medium`（如妊娠）只告警不中断。
- **payload 字段**：`herbs: string[]`、`pregnant: bool`（均可选）。

### 治疗 `treatment`
- **职责**：产出方剂 / 调护 / 外治 / 生活调摄建议。
- **实现**：
  1. 证候定位：`payload.syndrome`，缺省时用 `infer_syndrome_slug()` 取首位；
  2. 规则检索：`find_formula()` 拼 `【推荐方剂】`、`find_care()` 拼 `【调护建议】`；
  3. 用药安全：若 `payload.herbs` 存在，同上校验并拼 `【用药安全】`；
  4. LLM 综合：`chat_with_tools(system=prompts.treatment, cap=treatment)`；
  5. 输出 = LLM 部分 + 规则部分。
- **payload 字段**：`syndrome: string`、`herbs: string[]`、`pregnant: bool`（均可选）。

---

## 维护入口

| 想改什么 | 改哪里 | 是否需重启 |
|---|---|---|
| System Prompt（7 段） | `resources/prompts.yaml` | `POST /reload` 或重启 |
| 证候库（症状/病机） | `resources/syndromes.yaml` | 同上 |
| 问诊题库 | `resources/questions.yaml` | 同上 |
| 方剂 | `resources/formulas.yaml` | 同上 |
| 调护/食疗 | `resources/care.yaml` | 同上 |
| 红旗规则 | `resources/safety.yaml` | 同上 |
| 关键词→证据/证候映射 | `resources/keywords.yaml` | 同上 |
| 证候传变 | `resources/transformations.yaml` | 同上 |
| 流程顺序 | `resources/routing.yaml` | 同上 |
| Agent 逻辑 | `src/agents/*.rs` | 需 `cargo build` |

改完数据建议跑 `cd server && cargo test -p harness --test cases`
（93 条真实病例基准，会校验证候是否缺失、方剂/调护是否存在）。
