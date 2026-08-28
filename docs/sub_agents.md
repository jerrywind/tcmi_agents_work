# Sub-Agent 设计：职责、System Prompt 与技能

本文档说明各 Sub-Agent 的角色边界、面向 **`google/gemma-4-12b-qat`（文本+视觉共用，原生多模态）** 设计的 system prompt，
以及每个 agent 可调用的 **skill（工具）**。

后端已由 Python（原 `backend/`，归档于 `_useless/backend/`）重写为 Rust **harness**：
- 各 agent 实现：`server/harness/src/agents/*.rs`（`inspection.rs`、`listening.rs`、
  `inquiry.rs`、`palpation.rs`、`differentiation.rs`、`safety.rs`、`treatment.rs`）
- system prompt：`server/harness/resources/prompts.yaml`（**可改，中文维护**）
- 技能映射：`server/harness/src/skills/builtin.rs`（内置注册 9 个技能）
- 流程顺序：`server/harness/resources/routing.yaml`

> 关键约束：harness 中各 agent 统一走 **LLM** 实现（无 rule/mock 切换开关），
> 因此 `/chat` 需要真实 LLM（LM Studio 或 llm_server 网关）。
> 确定性逻辑（关键词证据匹配、证候推断、配伍禁忌、方剂调护检索）不依赖 LLM，
> 由 `cargo test -p harness --test cases` 覆盖。

## 总览

| 能力 (Capability) | 角色 | LLM 实现类 | 默认 impl | 专属技能 |
|---|---|---|---|---|
| `diagnosis.inspection` | 望诊 | `InspectionVisionAgent` (`llm_vision`) | rule | `tcm-vision`、`tcm-rag` |
| `diagnosis.listening` | 闻诊 | `ListeningLLMAgent` (`llm`) | rule | `tcm-auscultation` |
| `diagnosis.inquiry` | 问诊 | `InquiryLLMAgent` (`llm`) | rule | `tcm-inquiry`、`tcm-rag` |
| `diagnosis.palpation` | 切诊 | `PalpationLLMAgent` (`llm`) | rule | `tcm-palpation` |
| `diagnosis.differentiation` | 辨证 | `DifferentiationLLMAgent` (`llm`) | rule | `tcm-reference`、`tcm-rag` |
| `diagnosis.safety` | 安全 | `SafetyLLMAgent` (`llm`) | rule | `tcm-safety`（叠加规则兜底） |
| `treatment.plan` | 诊疗方案 | `TreatmentLLMAgent` (`llm`) | rule | `tcm-kb`、`tcm-diet`、`tcm-rag` |

> 每个能力现都至少绑定一个技能；`tcm-rag` 为多模态检索技能，由望/辨/施/问共享。
> 全部 LLM 实现均已接入 `run_tool_loop`，可在推理时调用各自技能。

## 各 Sub-Agent 设计

### 望诊 `diagnosis.inspection`
- **职责**：解读用户上传的舌象/面象/患处图片。
- **与模型关系**：唯一走 `llm_vision` 的能力；图片由 **`google/gemma-4-12b-qat`** 原生多模态端点理解（文本+视觉共用同一模型，无独立视觉服务），无需 mmproj。
- **System Prompt 要点**：仅描述可见事实（舌体/舌苔/舌态/面色/神/患处），不做诊断结论；
  输出契约 `{findings:[{part,value,confidence}], summary}`。
- **技能 `tcm-vision`**：模型可调用 `analyze_tongue_image(path)` / `analyze_face_image(path)`，
  由技能内部再次向 `llm_server` 发送图片并取回客观描述，再综合成结论。
- **降级**：无视觉模型时返回 `skip`，提示自然光下重拍舌象，由后续问诊补全。

### 闻诊 `diagnosis.listening`
- **职责**：从口语化自述中抽取声、嗅、咳、息线索。
- **System Prompt 要点**：输出 `{evidences:[{category,value,confidence}], notes}`，
  category 限定 `voice/odor/cough/breathing`，用中医习语（声低息微、口气酸臭等）。
- **降级**：抽取失败/无模型时回退到规则关键词（`KEYWORD_EVIDENCE`）。
- **技能 `tcm-auscultation`**：`lookup_voice_pattern(query)` / `lookup_odor_pattern(query)` 把口语化
  描述映射到声/嗅标准术语与病机，校准输出取值。

### 问诊 `diagnosis.inquiry`
- **职责**：基于已收集证据，提出下一个最有助于鉴别的问题并给选项。
- **System Prompt 要点**：category 限定 `sleep/diet/stool/fever/sweat/pain/emotion/menstruation/other`；
  优先追问能区分高概率候选证候的证据缺口；选项 2~6 个互斥。
- **技能 `tcm-inquiry`**：`lookup_inquiry_focus(syndrome)` 按候选证候给出最值得追问的特征；
  `suggest_followup(symptoms)` 按已采集症状反推候选证候并建议下一个最具鉴别力的追问。
  另可调用 `tcm-rag` 的 `rag_text_retrieve` 检索相似主诉以辅助收敛。

### 切诊 `diagnosis.palpation`
- **职责**：把用户自述的脉率、脉感、腹诊、肢体温度转写为证据。
- **System Prompt 要点**：category 限定 `pulse.rate/pulse.quality/abdomen/limb_temp`；
  对不准确表述做合理推断并标注置信度。
- **降级**：无模型时回退到自测心率规则（`PalpationRuleAgent`）。
- **技能 `tcm-palpation`**：`lookup_pulse_pattern(query)` / `lookup_abdomen_pattern(query)` 把脉感、
  腹诊、肢体温度等口语描述映射为标准中医术语与病机，校准输出取值。

### 辨证 `diagnosis.differentiation`
- **职责**：综合四诊证据给出候选证候及置信度（支持兼证）。
- **System Prompt 要点**：输出 JSON 数组 `{syndrome,confidence,evidence[]}`；
  证候用标准中医术语；只列置信度 ≥ 0.3 的候选。
- **技能 `tcm-reference`**：`lookup_syndrome_patterns(syndrome)` 查询某证候典型四诊表现，
  用于校准候选与支撑证据，减少臆造。

### 安全 `diagnosis.safety`（双重兜底）
- **职责**：识别红旗（red-flag）信号，提示线下就医。
- **设计要点**：**规则关键词扫描永远执行**作为强制安全网；LLM 在其上叠加语义识别
  （如「咯血」「剧烈胸痛」的口语化表述），二者合并去重。无模型时仅保留规则结果，绝不漏报。
- **输出契约**：`{safe, alerts:[{level:warning|urgent, signal, detail}]}`。
- **技能 `tcm-safety`**：`lookup_redflag(signal)` 把识别到的红旗信号映射为分级、建议就诊科室与处置要点，
  使告警更可操作；未命中具体信号时回退为通用就医提示（不漏报）。

### 诊疗方案 `treatment.plan`
- **职责**：基于证候生成调治建议（中药/针灸/西医检查/生活调护）。
- **System Prompt 要点**：输出 `{herbs, acupuncture, western, advice, questions}`；
  明确区分养生调护与需执业中医师处方的部分。
- **技能 `tcm-kb`**：`lookup_syndrome_treatment(syndrome)` / `lookup_herb(herb)` 查询知识库，
  优先据此给出有据可循的方案（见 [`skills.md`](./skills.md)）。
- **技能 `tcm-diet`**：`lookup_diet_therapy(syndrome)` 按证候返回食疗/膳食调护建议（宜食、忌口、机理）。
- **技能 `tcm-rag`**：`rag_text_retrieve` / `rag_paired_retrieve` 可检索治法/方剂出处或相关病例，
  进一步支撑方案生成。

## 如何调整诊断流程

编辑 `server/harness/resources/routing.yaml` 的 `active` 列表，即可增删诊断步骤
（删除某行即跳过该能力）：

```yaml
active:
  - inspection        # 望诊（多模态，与文本共用 google/gemma-4-12b-qat）
  - listening         # 闻诊
  - inquiry           # 问诊
  - palpation         # 切诊
  - differentiation   # 辨证
  - safety            # 安全门
  - treatment         # 治疗
default: inspection
```

改完调用 `POST /reload` 生效（需 `resources/config.yaml` 中 `hot_reload: true`），
或重启 harness。
