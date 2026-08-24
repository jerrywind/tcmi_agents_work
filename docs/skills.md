# SKILL 工具集（LLM 可调用技能）

> SKILL = 一组 **LLM 可调用工具（function calling）**。每个技能是一个独立模块，
> 声明一份清单 `SKILL` 与对应的处理函数 `HANDLERS`，装载后其工具即可被
> 「诊疗方案 / 辨证」等 LLM Agent 在推理时按需调用。

## 1. 架构总览

```
app/skills/
├── __init__.py          # 暴露 skill_registry / run_tool_loop / 装载函数
├── types.py             # SkillManifest / ToolSpec / SkillError
├── registry.py          # 全局单例 registry：注册/卸载/按能力过滤/执行
├── loader.py            # discover_skills / load_skill_by_name / load_skill_from_path
├── toolcall.py          # run_tool_loop：多轮工具调用循环
├── tcm-kb/              # 内置技能：中医知识库（施治调用）
│   └── __init__.py
├── tcm-reference/       # 内置技能：证候典型表现检索（辨证调用）
│   └── __init__.py
├── tcm-vision/          # 内置技能：望诊图文理解（Qwen3-VL，望诊调用）
│   └── __init__.py
├── tcm-diet/            # 内置技能：辨证食疗/膳食调护（施治调用）
│   └── __init__.py
├── tcm-auscultation/    # 内置技能：闻诊声/嗅术语参照（闻诊调用）
│   └── __init__.py
├── tcm-palpation/       # 内置技能：切诊脉/腹诊术语参照（切诊调用）
│   └── __init__.py
├── tcm-safety/          # 内置技能：红旗信号分诊指引（安全调用）
│   └── __init__.py
├── tcm-inquiry/         # 内置技能：问诊追问聚焦（问诊调用）
│   └── __init__.py
└── tcm-rag/             # 内置技能：多模态 RAG 检索（望/辨/施/问共享）
    └── __init__.py
```

- **Registry（单例）**：进程内唯一，保存「技能 → 工具」映射，提供
  `register_skill / unload / tools_for(capability) / run_tool / list_skills / list_tools`。
- **Loader**：从文件系统发现并导入技能模块；支持
  - `discover_skills(dir)`：启动自动发现；
  - `load_skill_by_name(name, skills_dir)` / `load_skill_from_path(path)`：运行时热装载。
- **Tool-calling Loop**：`run_tool_loop(provider, messages, model, capability)`。
  当某能力下注册了工具时，驱动 LLM 多轮调用工具，最后以 `json_mode` 产出结构化结果；
  **无工具时退化为单次 `json_mode` 调用，与既有行为完全兼容**。

## 2. 技能模块契约

一个技能模块（包或 `.py` 文件）必须定义：

```python
from app.skills.types import SkillManifest, ToolSpec

SKILL = SkillManifest(
    name="my-skill",
    version="0.1.0",
    description="一句话说明这个技能能做什么",
    tools=[
        ToolSpec(
            name="lookup_xxx",
            description="LLM 看到的自然语言描述（要清晰说明何时调用）",
            parameters={            # JSON Schema
                "type": "object",
                "properties": {"q": {"type": "string", "description": "查询参数"}},
                "required": ["q"],
            },
            capability="treatment.plan",   # 该工具可被哪个能力调用；"" = 全部能力
        ),
    ],
)

async def lookup_xxx(q: str) -> dict:
    return {"ok": True, "result": ...}

HANDLERS = {"lookup_xxx": lookup_xxx}
```

要点：
- `SKILL.tools` 中每个 `name` 必须在 `HANDLERS` 中有对应可调用对象，否则装载抛 `SkillError`。
- 处理函数可为同步或 `async`；`run_tool` 会自动 `await`。
- `capability` 为空字符串表示对所有能力开放；否则仅对指定 `Capability` 值（如
  `treatment.plan`、`diagnosis.differentiation`）开放。**也支持 `list[str]`**，
  表示该工具对列表中任一能力开放（如 RAG 检索工具被多个能力共享）。
- **技能模块使用绝对导入**（`from app.skills.types import ...`），以便装载器从任意路径
  用 `importlib` 加载时相对导入不会失效。

## 3. 内置技能 `tcm-kb`（施治调用）

随系统启动自动装载，演示工具调用：

| 工具 | 能力 | 说明 |
|---|---|---|
| `lookup_syndrome_treatment` | `treatment.plan` | 按证候名返回推荐的多模态诊疗方案（方剂/针灸/外治/检查/调护） |
| `lookup_herb` | `treatment.plan` | 按中药名查询性味归经与功效主治 |

当 LLM（诊疗方案 Agent）在 `treatment.plan` 能力下运行时，会自动获得这两个工具，
可在生成方案前先查证知识库，从而产出更可靠的方案。

### 3.1 内置技能 `tcm-reference`（辨证调用）

| 工具 | 能力 | 说明 |
|---|---|---|
| `lookup_syndrome_patterns` | `diagnosis.differentiation` | 按证候名返回典型四诊表现（`category/value` 列表），用于校准辨证结论 |

「辨证」LLM 在 `diagnosis.differentiation` 能力下运行时会自动获得该工具，可查询某证候
（如 `肝郁脾虚`、`脾胃湿热`）的典型表现以校验候选证候与支撑证据，减少臆造。

### 3.2 内置技能 `tcm-vision`（望诊调用，依赖 Qwen3-VL 视觉模型）

| 工具 | 能力 | 说明 |
|---|---|---|
| `analyze_tongue_image` | `diagnosis.inspection` | 分析舌象图片，返回舌体/舌苔/舌态客观观察 |
| `analyze_face_image` | `diagnosis.inspection` | 分析面象/神色图片，返回面色/神色客观观察 |

该技能依赖独立的 **Qwen3-VL** 视觉服务（原生多模态，无需 mmproj）：处理函数把图片以 data-URL
形式发送到视觉端点并取回描述文字。「望诊」LLM（`llm_vision`）在推理时可决定对哪张图片调用
`analyze_tongue_image` / `analyze_face_image`，再综合成结构化望诊结论。无视觉模型时
处理函数返回错误标记，由望诊 Agent 降级为 `skip`。详见 [`llm_server.md`](./llm_server.md)。

### 3.3 内置技能 `tcm-diet`（施治调用）

| 工具 | 能力 | 说明 |
|---|---|---|
| `lookup_diet_therapy` | `treatment.plan` | 按证候名返回食疗/膳食调护建议（宜食、忌口、机理）；命中不到时回退通用原则 |

为「施治」LLM 提供个性化饮食建议的检索支撑；提示文案标明食疗为养生参考，具体体质须经执业中医师辨证。

### 3.4 内置技能 `tcm-auscultation`（闻诊调用）

| 工具 | 能力 | 说明 |
|---|---|---|
| `lookup_voice_pattern` | `diagnosis.listening` | 检索语声/呼吸/咳嗽相关标准术语与病机 |
| `lookup_odor_pattern` | `diagnosis.listening` | 检索气味相关标准术语与病机 |

把口语化描述映射到标准中医术语，帮助「闻诊」LLM 输出一致、可解释的证据取值（支持字符重叠模糊匹配）。

### 3.5 内置技能 `tcm-palpation`（切诊调用）

| 工具 | 能力 | 说明 |
|---|---|---|
| `lookup_pulse_pattern` | `diagnosis.palpation` | 检索脉象相关标准术语与主病/含义 |
| `lookup_abdomen_pattern` | `diagnosis.palpation` | 检索腹诊/触诊相关标准术语与含义 |

把脉感、腹诊、肢体温度等口语描述映射为标准中医术语，校准「切诊」证据。

### 3.6 内置技能 `tcm-safety`（安全调用）

| 工具 | 能力 | 说明 |
|---|---|---|
| `lookup_redflag` | `diagnosis.safety` | 把红旗信号映射为分级（warning/urgent）、建议就诊科室与处置要点 |

让「安全」LLM 的告警更具可操作性；未命中具体信号时回退为通用就医提示（不漏报）。

### 3.7 内置技能 `tcm-inquiry`（问诊调用）

| 工具 | 能力 | 说明 |
|---|---|---|
| `lookup_inquiry_focus` | `diagnosis.inquiry` | 按候选证候返回最值得追问的特征（问题文案、选项、典型取值） |
| `suggest_followup` | `diagnosis.inquiry` | 按已采集症状反推候选证候并建议下一个最具鉴别力的追问 |

帮助「问诊」LLM 收敛提问方向、避免重复采集。

### 3.8 内置技能 `tcm-rag`（望/辨/施/问共享，依赖 RAG 服务）

| 工具 | 能力 | 说明 |
|---|---|---|
| `rag_text_retrieve` | `diagnosis.differentiation` + `treatment.plan` + `diagnosis.inquiry` | 文本检索（以文搜文） |
| `rag_image_retrieve` | `diagnosis.inspection` + `diagnosis.differentiation` | 图像检索（以图搜图） |
| `rag_paired_retrieve` | `diagnosis.differentiation` + `treatment.plan` + `diagnosis.inspection` | 图文联合检索（以文搜图/以图搜文） |

对接 `llm_server` 中已构建的 RAG 服务（见 [`rag.md`](./rag.md)），为多个子智能体提供可检索的
多模态中医知识库。通过 `TCM_RAG_BASE_URL` 配置服务地址；**服务不可用时工具优雅降级**
（`ok=false`、空结果），不阻断诊疗流程。

## 4. 装载方式（两者都支持）

### 4.1 启动自动发现
应用导入 `app.main` 时执行 `discover_skills(SKILLS_DIR)`，扫描 `skills/` 目录下的
所有技能包/模块并注册。技能目录默认 `backend/app/skills`，可用环境变量覆盖：

```bash
export TCM_SKILLS_DIR=/path/to/your/skills
```

### 4.2 运行时热装载 / 卸载（API）
无需重启即可增删技能：

```bash
# 列出当前已装载技能与工具
GET /api/skills

# 按名称装载 skills/ 下的技能
POST /api/skills/load
{ "name": "tcm-kb" }

# 按文件路径装载（目录或 .py；支持绝对/相对路径）
POST /api/skills/load
{ "path": "/abs/path/to/skill" }

# 卸载技能（移除其全部工具）
POST /api/skills/unload
{ "name": "tcm-kb" }
```

- 装载失败（清单缺失、工具无 handler、模块导入错误）返回 `400` 并给出原因；
- 卸载不存在的技能返回 `404`。
- 同名技能热装载会先卸载旧版再注册新版（避免工具残留）。
- 前端「管理技能 / SKILL」页（`pages/skills/index`）已封装上述接口。

### 4.3 接入外部 MCP Server 的工具

除本地技能外，还可把**外部 MCP Server** 的工具接入本系统。连接成功后，
其工具会以 `mcp__<连接名>__<工具名>` 注册进同一个 SKILL 注册表，
对 LLM 而言与本地技能完全一致（默认对所有 capability 开放）。

```bash
# 运行时接入
POST /api/mcp/clients
{ "name": "weather", "transport": "http", "url": "http://localhost:9001/mcp" }

# 断开并卸载其全部工具
DELETE /api/mcp/clients/weather
```

也可写进 `routing.yaml` 的 `mcp.clients` 随应用启动自动连接。
详见 [`MCP 集成`](./mcp.md)。

## 5. LLM 如何调用技能

全部 7 个子智能体的 LLM 实现（望 `InspectionVisionAgent`、闻 `ListeningLLMAgent`、问 `InquiryLLMAgent`、
切 `PalpationLLMAgent`、辨证 `DifferentiationLLMAgent`、安全 `SafetyLLMAgent`、施治 `TreatmentLLMAgent`）
在生成结果前调用 `run_tool_loop(provider, [user_msg], model, capability)`：

1. `tools = skill_registry.tools_for(capability)`；无工具则单次 `json_mode` 调用（兼容旧行为）。
2. 有工具则把工具声明传给 `provider.chat(..., tools=tools)`，解析返回的 `tool_calls`。
3. 对每个 `tool_call` 调用 `skill_registry.run_tool(name, args)`，将结果以 `role: "tool"`
   回填对话，再让 LLM 继续；直到无工具调用或达到 `max_tool_rounds`（默认 3）。
4. 最后以 `json_mode=True` 产出最终结构化结果。

> 各能力可调用哪些技能见 `app/agents/skills_map.py`（与 [`sub_agents.md`](./sub_agents.md) 一致）：
> - `diagnosis.inspection` → `tcm-vision`、`tcm-rag`
> - `diagnosis.listening` → `tcm-auscultation`
> - `diagnosis.inquiry` → `tcm-inquiry`、`tcm-rag`
> - `diagnosis.palpation` → `tcm-palpation`
> - `diagnosis.differentiation` → `tcm-reference`、`tcm-rag`
> - `diagnosis.safety` → `tcm-safety`
> - `treatment.plan` → `tcm-kb`、`tcm-diet`、`tcm-rag`

> 工具执行异常会被捕获并作为 `{"error": ...}` 回填，不会击穿整个流程。

## 6. 编写你自己的技能

1. 在 `backend/app/skills/` 下新建目录（或 `.py` 文件），如 `my-skill/__init__.py`；
2. 按第 2 节契约定义 `SKILL` 与 `HANDLERS`；
3. 重启服务（自动发现）或 `POST /api/skills/load {"name": "my-skill"}` 热装载；
4. 在 Prompt 中（或依赖 `capability` 过滤）让 LLM 知道何时调用你的工具。

## 7. 测试

- `tests/test_skills.py`：清单/工具注册、`tools_for` 能力过滤、同步/异步执行、卸载、
  目录发现、错误分支（清单缺失 / 缺 handler / 按名装载）。
- `tests/test_skill_api.py`：列表、热装载、热卸载、错误分支（缺参数 400 / 未知 404）。
- `tests/test_llm_agents.py`：扩展 `OpenAICompatProvider.chat` 的工具调用路径，
  以及 `run_tool_loop` 的多种分支（无工具回退 / 执行工具 / 空工具调用 / 轮次耗尽 /
  工具模式下直接返回文本）。
