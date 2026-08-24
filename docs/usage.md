# 使用文档（Usage）

面向两类使用者：**终端用户**（通过前端页面完成问诊）与**接入方**（通过 REST API 集成）。

> ⚠️ 免责声明：本系统由 AI 生成，仅供健康参考，**不构成医疗诊断或处方建议**。如有不适或红旗症状，请及时线下就医。

## 1. 产品使用流程（前端页面）

1. **新建问诊档案**（`pages/index`）
   - 填写基本信息：常住地、身高、体重、年龄、性别。
   - 填写病情自述（主诉）。
   - 上传舌象（建议自然光、伸舌平展拍摄），面相 / 患处照片可选。
2. **多轮问诊**（`pages/consult`）
   - 顶部进度条依次显示「望 → 闻 → 问 → 切 → 治」。
   - AI 按望闻问切流程逐步提问，多为**选项卡片 + 自由文本**，降低输入成本。
   - 候选证候范围会随回答逐步收窄（页面用进度可视化）。
   - 出现红旗症状（如胸痛、咯血、高热不退）会被安全机制中断并引导就医。
3. **诊疗方案**（`pages/consult` 末端 + `pages/report`）
   - 辨证完成后，会就个别情况追问 1~2 条（如是否方便煎药、是否接受针灸/西医检查、是否孕期/备孕）。
   - 产出以「更快、更彻底痊愈」为目标的综合方案，涵盖：**中药方剂、针灸推拿、外治法、西医检查、生活调护/膳食**。
   - 西医检查项用于明确诊断、排除器质病变（用户暂拒则降级为可选建议）。
4. **诊断报告**（`pages/report`）
   - 证候结论（1~2 条）、辨证依据链（支持/矛盾证据）、调理建议、诊疗方案、免责声明，支持查看与分享。

5. **技能管理**（`pages/skills`，可选）
   - 列出当前已装载的 SKILL 技能及其工具清单（如 `tcm-kb`、`tcm-rag` 等）。
   - 可按名称装载 `skills/` 目录下的技能，或卸载已装载技能，无需重启后端服务。
   - 适合在扩展中医知识库 / 接入自有 RAG 后，动态启用新能力。详见 [`SKILL 工具集`](./skills.md)。

## 2. REST API 接入（接入方）

Base URL（开发）：`http://localhost:8000/api`

### 2.1 新建档案
```bash
curl -X POST http://localhost:8000/api/consultations \
  -H 'Content-Type: application/json' \
  -d '{
    "patient": {"region":"广州","height_cm":172,"weight_kg":78,"age":34,"gender":"男"},
    "complaint": "口苦口臭、大便粘滞不爽、肢体困重",
    "self_report": {"heart_rate": 76}
  }'
# -> { "id": "cons_xxx", "status": "created" }
```

### 2.2 上传舌象/面相/患处照片
```bash
curl -X POST http://localhost:8000/api/consultations/cons_xxx/images \
  -F 'type=tongue' \
  -F 'file=@tongue.jpg'
# type 取值：tongue | face | lesion
# -> { "id": "img_xxx", "url": "/uploads/cons_xxx_xxxx.jpg" }
```

### 2.3 启动诊断
```bash
curl -X POST http://localhost:8000/api/consultations/cons_xxx/start
# -> StateResp: status=waiting_answer, question={...}
```

### 2.4 回答（辨证追问 或 方案个性化追问）
```bash
curl -X POST http://localhost:8000/api/consultations/cons_xxx/answer \
  -H 'Content-Type: application/json' \
  -d '{"question_id":"q_xxx","value":"可煎药"}'
# 返回 StateResp：可能继续 waiting_answer / treatment_qa，或 finished
```

### 2.5 拉取状态 / 报告 / 调用轨迹
```bash
curl http://localhost:8000/api/consultations/cons_xxx           # 当前状态与消息流
curl http://localhost:8000/api/consultations/cons_xxx/report    # 诊断+诊疗方案报告
curl http://localhost:8000/api/consultations/cons_xxx/trace     # 各 Sub-Agent 调用明细
curl http://localhost:8000/api/system/agents                    # 当前路由（谁在用哪个实现/模型）
```

### 2.6 快速接入：Postman / OpenAPI

- **Postman 集合**：`docs/tcm-agent.postman_collection.json`，已包含全部端点与示例请求、变量 `baseUrl`/`cid`，导入 Postman 即可联调。
- **OpenAPI**：服务启动后自动生成，访问 `http://<host>:8000/openapi.json`，Swagger UI 在 `/docs`、Redoc 在 `/redoc`。

### 2.6.x 技能管理接口（SKILL）
```bash
curl http://localhost:8000/api/skills                      # 列出已装载技能与工具
curl -X POST http://localhost:8000/api/skills/load \
  -H 'Content-Type: application/json' -d '{"name":"tcm-kb"}'  # 按名装载
curl -X POST http://localhost:8000/api/skills/unload \
  -H 'Content-Type: application/json' -d '{"name":"tcm-kb"}'  # 卸载
# 也支持按路径装载：{"path":"/abs/path/to/skill"}
```

### 2.6.y MCP 接口（对外暴露能力 / 接入外部工具）
```bash
curl http://localhost:8000/api/mcp/status                  # 挂载状态、外部连接、各能力实现
curl http://localhost:8000/api/mcp/tools                   # 本 MCP Server 的全部工具及 schema

# 运行时接入一个外部 MCP Server（其工具变为 mcp__weather__*）
curl -X POST http://localhost:8000/api/mcp/clients -H 'Content-Type: application/json' \
  -d '{"name":"weather","transport":"http","url":"http://localhost:9001/mcp"}'
curl -X DELETE http://localhost:8000/api/mcp/clients/weather   # 断开并卸载
```
MCP 客户端（Claude Desktop / Cursor 等）可直接连接 `http://localhost:8000/mcp`
（Streamable HTTP），或用 stdio：`cd backend && python -m app.mcp.server`。
详见 [`MCP 集成`](./mcp.md)。
> 完整契约与错误码见 [`SKILL 工具集`](./skills.md) 第 4 节。

### 2.7 字段速览（响应）
- `StateResp.status`：`created | running | waiting_answer | planning | treatment_qa | finished | referred`
- `report.syndromes[]`：证候（1~2 条）+ 置信度 + 支持/矛盾证据
- `report.treatments[]`：诊疗方案（`category`/`title`/`detail`/`rationale`/`note`/`priority`）
- `report.red_flag`：非 null 时表示触发红旗告警（建议立即就医）

## 3. 切换 / 启用真实 LLM

编辑 `backend/app/routing.yaml`（或指向 `routing.llm.yaml` 一键切换）：

```yaml
routing:
  diagnosis.inspection:        { impl: llm_vision, model: vision-default }  # 望诊多模态分析
  diagnosis.differentiation:  { impl: llm, model: text-default }           # 大模型辨证
  diagnosis.inquiry:          { impl: llm, model: text-default }           # 大模型提问
  treatment.plan:             { impl: llm, model: text-default }           # 大模型生成综合方案
llm:
  base_url: ""                 # 由 TCM_LLM_BASE_URL 注入（文本端点）
  api_key_env: TCM_LLM_API_KEY
  models:
    text-default: qwen3.6-9B          # 文本问诊（听/问/切/辨证/安全/施治）
    vision-default: Qwen3-VL-8B       # 望诊视觉模型（原生多模态）
```
> 顶层键为 `routing`（非 `routes`）。启用全部 LLM 实现最简：`TCM_ROUTING_FILE` 指向
> `routing.llm.yaml`（compose 的 `llm` profile 已设好）。无模型可用时所有 LLM 实现会自动
> 降级为 rule/mock，保证系统可用。
设置环境变量 `TCM_LLM_API_KEY` 后重启后端即可。未配置时自动降级 mock，全流程仍可演示。

### 3.1 本地开发最简易：用 LM Studio（无需权重/Docker）

LM Studio 加载任意多模态模型（如 `google/gemma-4-12b-qat`，文本与视觉共用同一端点），
开启本地服务器（默认 `http://localhost:11223/v1`），然后设置：

```bash
# PowerShell
$env:TCM_LLM_BASE_URL="http://localhost:11223/v1"
$env:TCM_LLM_API_KEY="<LM Studio → Developer → Server Settings 中的 API Key>"
$env:TCM_LLM_TEXT_MODEL="google/gemma-4-12b-qat"
$env:TCM_LLM_VISION_MODEL="google/gemma-4-12b-qat"
$env:TCM_LLM_API="responses"          # 使用 LM Studio Responses API (/v1/responses)
$env:TCM_ROUTING_FILE="app/routing.llm.yaml"
```

> 视觉与文本复用同一端点即可（gemma-4 为原生多模态）。`routing.llm.yaml` 默认 `api: responses`，
> 即通过 LM Studio 的 **Responses API** 调用；若需回退到传统 Chat Completions，设 `TCM_LLM_API=chat`
> 或把 `routing.llm.yaml` 的 `llm.api` 改为 `chat`。若 LM Studio 开启了 API Key 校验，需填入对应
> Key（Developer → Server Settings → API Key）；关闭校验则任意非空值均可。

## 4. 常见问题

- **为什么有时中途直接结束？** 若为 `referred`，说明出现红旗症状，系统优先保障安全并引导线下就医。
- **为什么只问了很少几轮？** 当候选证候置信度达标（Top1≥0.55 且领先≥0.15）或接近兼证时即收敛，避免无谓打扰。
- **方案里出现西医检查？** 这是"更快更彻底痊愈"的主动设计：用西医手段明确诊断、排除器质病变，与中医方案互补。
- **孕期/备孕提示？** 方案阶段会追问，若选"孕期/备孕"则中药项会附加安全提示，用药须由专业医师辨证。
