# Sub-Agent 设计：职责、实现与 Prompt

本文档描述 13 个 Sub-Agent 各自的职责边界、**规则层与 LLM 层的组合方式**，
以及 prompt 与知识数据的维护位置。

> **与典籍检索的结合**：每个 Sub-Agent 都有**自己的检索域**，用典籍分类的
> 四维标签（临床学科 / 内容体裁 / 功能用途 / 学术流派）圈定，见
> [`rag_scopes.yaml`](../server/harness/resources/rag_scopes.yaml) 与
> [`rag.md`](./rag.md#post-ragretrievescope按知识域检索)。
> 切诊翻《脉经》、开方翻《普济方》，互不稀释。

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

| 期 | capability | 角色 | 实现类 | 规则层 | 检索域（体裁 / 功能） |
|---|---|---|---|---|---|
| 采集 | `inspection` | 望诊 | `InspectionAgent` | 后置：关键词证据叠加 | 诊断学 / 诊断方法 |
| 采集 | `listening` | 闻诊 | `ListeningAgent` | 后置：关键词证据叠加 | 临床各科 / 专科证治 |
| 采集 | `inquiry` | 问诊 | `InquiryAgent` | 后置：`questions.yaml` 未覆盖问题去重取 top6 | 诊断学、入门歌诀 |
| 采集 | `palpation` | 切诊 | `PalpationAgent` | 后置：`parse_ppg` 体检数值解析 | 诊断学 / 诊断方法 |
| 辨证 | `case_reference` | 医案参考 | `CaseReferenceAgent` | 前置：候选证候作检索靶点 | 临证医案 / 临证实录 |
| 辨证 | `differentiation` | 辨证 | `DifferentiationAgent` | **前置**：证候库结构化打分（主证/兼证 + 置信度 + 支持/矛盾证据）+ 传变提示；并产出 `structured` | 医经基础、伤寒论、金匮要略、医话医论 |
| 安全 | `safety` | 安全门 | `SafetyAgent` | **前置**（强制）：红旗扫描 + 用药禁忌 | 本草药物 / 配伍归经、药性理论 |
| 治疗 | `strategy` | 立法 | `StrategyAgent` | 前置：证候 + 病机 + `principles` 治则 | 临床综合、医话医论 |
| 治疗 | `herbology` | 用药 | `HerbologyAgent` | 前置：候选药物 + 配伍/妊娠禁忌校验 | 本草药物 / 药性理论、炮制制剂、配伍归经 |
| 治疗 | `prescription` | **开方（最后一步）** | `PrescriptionAgent` | 前置：`formulas.yaml` 方剂检索 | 方书方剂 / 方剂汇编、专科方书、经验验方、成药标准、急救方书 |
| 治疗 | `care` | 调护 | `CareAgent` | 前置：`care.yaml` 调护检索 | 养生摄生 / 养生调摄、食疗本草 |
| 治疗 | `acupuncture` | 针灸外治 | `AcupunctureAgent` | 无 | 针灸经络 / 刺法灸法、腧穴考证、经络理论、时间针法、推拿按摩 |
| 兼容 | `treatment` | 治疗（旧流程） | `TreatmentAgent` | 前置：方剂/调护检索 + 用药安全 | 方书方剂、临床综合 |

> **采集期四步并行**执行（互不依赖），随后进入辨证期。
> 治疗期默认只跑「立法 → 用药 → 开方」；调护与针灸在 `full` 档位才启用。

「3 全局」= `tcm-kb`、`tcm-diet`、`tcm-rag`；若 `config.yaml` 配了 `mcp_clients`，
每个 Agent 还会额外看到全部 `mcp__*` 工具。见 [`skills.md`](./skills.md) 第 2.1 节。

> **工具调用已在链路中生效**：7 个 Agent 全部走 `chat_with_tools`，上表「可见技能」
> 即推理中模型真正能调用的工具，调用轨迹见 `/chat` 响应的 `trace[].tool_calls`。
> 另：`safety` 命中 `high`/`critical` 红旗时，编排器会**跳过其后的步骤**（默认即
> `treatment`），见下文安全门一节与 [`usage.md`](./usage.md) 2.3。

---

## 两阶段反馈式辨证

患者常常只说「咳嗽两天」，此时硬辨证置信度只有 0.2，却照样往下走到开方——
开出来的方自然不可靠。真实中医靠**反复追问**把信息补齐，这里把它自动化：

```text
Phase A 采集（望闻问切，并行）
        ↓
Phase B 辨证（医案参考 → 辨证）
        ↓
   收敛判定 ── 不收敛 ──► 返回 status=awaiting_input + pending_questions
        │                          │
        │                          └── 用户回答后，前端把答案追加进 messages，
        │                              payload.round +1，再请求一次
        │ 收敛（或已达轮次上限）
        ↓
Phase C 安全门 → 立法 → 用药 → 开方
```

### 收敛三条件（同时满足才算收敛）

| 条件 | 默认 | 含义 |
|---|---|---|
| `confidence ≥ min_confidence` | 0.6 | 主证证据够不够 |
| `margin ≥ margin` | 1.0 | 主证与次证的证据量差——咬得很近说明鉴别不清 |
| `coverage ≥ coverage` | 0.8 | 必采信息（舌象/脉象/寒热等）采集到了没有 |

**兜底**：`round ≥ max_rounds`（默认 3）强制放行，标 `forced: true`、`converged: true`
——保证「最终一定有结论」，而不是把用户卡在无限追问里。

### 追问的三个来源（全部确定性，不靠 LLM 编造）

| 优先级 | 来源 | 依据 |
|---|---|---|
| 1 | **鉴别追问** | 主证与次证的**症状差集**自动推导。「想喝水吗」能区分风热与风寒，这正是鉴别诊断的精髓 |
| 2 | 未覆盖追问 | `questions.yaml` 中 `evidence_keys` 未命中对话的题目，按 `priority` |
| 3 | 证候补全 | 主证 `symptoms` 中尚未提及的典型表现 |

### 后续轮只跑必要的采集 agent

`questions.yaml` 每条带 `agent` 字段，标明该信息该由哪个采集 agent 负责。
首轮四诊全跑；后续轮若只剩「舌苔什么颜色」，就只跑望诊。

### 响应结构

```jsonc
{
  "status": "awaiting_input",        // 或 "completed"
  "loop": {
    "round": 1, "converged": false, "forced": false,
    "confidence": 0.4, "margin": 0.5, "coverage": 0.5,
    "primary": "wind_cold_attack_lung",
    "pending_questions": [
      {"slug": "tongue", "text": "方便看一下舌象吗？…",
       "reason": "尚缺「舌象」方面的信息", "source": "uncovered",
       "agent": "inspection", "priority": 1}
    ]
  },
  "steps": [...], "structured": {...}
}
```

`status=awaiting_input` 时**不会**执行安全门与治疗期步骤（也不会开方）。

### 两种跳过判定的情况

- `payload.syndrome` 已给定证候 —— 那是「已知证候求方剂」的场景，再追问是打扰；
- 流程里没有辨证期步骤（如 `routing.yaml` 只配了采集 + 治疗）。

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

### 医案参考 `case_reference`
- **职责**：在 48 部临证医案里检索**相似病案**，供辨证参照。
- **为什么单独一步**：医案是「别人的病历」，与「辨证规则」是两类东西。
  混进辨证步会让模型把个案当通则；单独一步则能在辨证**之前**给出参照。
- **实现**：规则层拼出「检索靶点」（已知/待鉴别证候）附进系统提示 → LLM 检索并摘录。
- **科室动态注入**：辨证出儿科，就看儿科医案。

### 立法 `strategy`
- **职责**：确立治则治法（汗、吐、下、和、温、清、消、补，可相兼）。
- **为什么单独一步**：「理法方药」里「法」承上启下——没有治则就直接开方，
  模型容易跳到具体方名而说不清为什么用它。有了治则，开方便有标尺可校验。
- **实现**：规则层取主证的 `principles`（治则，来自 `syndromes.yaml`）
  + `pathogenesis`（病机）→ LLM 展开立法依据。

### 用药 `herbology`
- **职责**：药性、炮制、配伍、剂量。
- **实现**：规则层给出候选方剂的药物组成 + 配伍/妊娠禁忌校验 → LLM 说明。
- **安全约束**：古籍剂量与炮制法带有时代局限，引用时必须注明「古法，今用须
  遵医嘱并以现代药典为准」，不得直接照搬古人剂量（写在 prompt 里）。

### 开方 `prescription`（**治疗阶段最后一步**）
- **职责**：据证候与治则给出主方（方名/组成/剂量/煎服法）与备选方。
- **实现**：规则层取 `formulas.yaml` 的确定性方剂 → LLM 再用 `tcm-rag`
  在 110 部方书里检索补充，说明取舍。
- **检索域**：方书方剂 ×（方剂汇编 / 专科方书 / 经验验方 / 成药标准 /
  急救方书 / 方论阐释），**科室随辨证结果动态收窄**——辨证出妇科就看妇科方书。
- **与本地库的关系**：`formulas.yaml` 只有 7 个方，是确定性兜底；
  真正的覆盖面来自方书检索。

### 调护 `care`（默认关闭）
- **职责**：饮食宜忌、起居作息、情志调摄、简易食疗方。
- **实现**：规则层取 `care.yaml` → LLM 用养生摄生与食疗本草补充。

### 针灸外治 `acupuncture`（默认关闭）
- **职责**：据证候与经络辨证给出取穴（主穴/配穴）、刺灸法与疗程。
- **实现**：无规则层（取穴高度依赖具体辨证），由模型在针灸典籍里检索后综合。

### 治疗 `treatment`（兼容旧流程）
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
| 相反表现（矛盾证据） | `resources/contradictions.yaml` | 同上 |
| 证候传变 | `resources/transformations.yaml` | 同上 |
| 证候治则（立法依据） | `resources/syndromes.yaml` 的 `principles` | 同上 |
| 证候对应科室（动态检索域） | `resources/syndromes.yaml` 的 `departments` | 同上 |
| 各 agent 的典籍检索域 | `resources/rag_scopes.yaml` | 同上 |
| 流程顺序与档位 | `resources/routing.yaml` 的 `profiles` / `active_profile` | 同上（`safety` 不可移除，缺失会被强制补齐） |
| 收敛阈值（置信度/鉴别度/覆盖率/轮次） | `src/agents/convergence.rs` 的 `LoopConfig::default()` | 需重新 `docker build` |
| Agent 逻辑 | `src/agents/*.rs` | 需重新 `docker build`（后端走 Docker） |

改完数据建议在 Docker 内跑 `--test cases` 回归
（93 条真实病例基准，会校验证候是否缺失、方剂/调护是否存在）。
