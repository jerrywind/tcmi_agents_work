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
   - 列出当前可用技能及其归属（`GET /skills`）：`tcm-kb`、`tcm-diet`、`tcm-rag` 等共 9 个。
   - harness 的技能为**内置注册**（Rust 编译期装配），不支持运行时按名装载/卸载
     （区别于原 backend 的 `skills/` 目录热装载）；新增技能需改 `skills/builtin.rs` 后重新构建。
   - 扩展知识库请优先改 `resources/*.yaml`，或用 `HARNESS_RAG_ENDPOINT` 接入自有 RAG。
     详见 [`SKILL 工具集`](./skills.md)。

## 2. REST API 接入（接入方）

Base URL（开发）：`http://localhost:8011`
（生产经 nginx 时为 `https://<域名>/api`，nginx 会剥离 `/api` 前缀后转发）

> **与原 backend 的重要差异**：harness 是**无状态**服务，不保存问诊会话，
> 没有 `cons_xxx` 会话 id、没有 `start/answer/report/trace` 等会话端点、也不提供
> 图片上传与 `/uploads` 静态目录（图片以 base64 / URL 随请求传入）。
> 多轮问诊由**调用方**（前端）维护 `messages` 数组，每次带上完整对话历史。

### 2.1 健康检查
```bash
curl http://localhost:8011/health          # -> ok
```

### 2.2 列出能力（Sub-Agent）
```bash
curl http://localhost:8011/agents
# -> {"capabilities":["inspection","listening","inquiry","palpation",
#     "differentiation","safety","treatment"],
#     "names":["望诊","闻诊","问诊","切诊","辨证","安全门","治疗"]}
```

### 2.3 完整诊断流程（推荐）
```bash
curl -X POST http://localhost:8011/chat \
  -H 'Content-Type: application/json' \
  -d '{
    "messages": [
      {"role":"user","content":"口苦口臭、大便粘滞不爽、肢体困重，舌红苔黄腻"}
    ],
    "payload": {"gender":"男","age":34,"region":"广州"}
  }'
# -> {"steps":[{"capability":"inspection","text":"..."}, ...],
#     "summary":"## 望诊\n...\n\n## 辨证\n..."}
```
按 `resources/routing.yaml` 的 `active` 顺序依次调用各 Sub-Agent（望→闻→问→切→辨证→安全门→治疗），
返回每一步的输出 `steps` 与汇总文本 `summary`。

### 2.4 单步调用某个 Sub-Agent
```bash
curl -X POST http://localhost:8011/agents \
  -H 'Content-Type: application/json' \
  -d '{
    "capability": "differentiation",
    "messages": [{"role":"user","content":"口苦口臭、肢体困重、舌红苔黄腻"}],
    "payload": {}
  }'
# -> {"capability":"differentiation","content":"..."}
```
`capability` 取值：`inspection` | `listening` | `inquiry` | `palpation` |
`differentiation` | `safety` | `treatment`。

### 2.5 技能（SKILL）
```bash
curl http://localhost:8011/skills           # 列出 9 个技能及归属（owner）
curl -X POST http://localhost:8011/skills \
  -H 'Content-Type: application/json' \
  -d '{"name":"tcm-kb","arguments":{"q":"脾胃湿热 常用方"}}'
# -> {"result": ...}
```
内置技能：`tcm-vision`(望诊)、`tcm-auscultation`(闻诊)、`tcm-inquiry`(问诊)、
`tcm-palpation`(切诊)、`tcm-reference`(辨证)、`tcm-safety`(安全门)、
`tcm-kb`、`tcm-diet`、`tcm-rag`(全局)。详见 [`SKILL 工具集`](./skills.md)。

### 2.6 热重载 YAML 资源
```bash
curl -X POST http://localhost:8011/reload   # -> {"ok":true}
```
需 `resources/config.yaml` 中 `hot_reload: true`。改完证候/方剂/问诊等 YAML 后调用即可，
无需重启（详见 [`deployment.md`](./deployment.md) 3.4）。

### 2.7 字段速览
- `Message`：`{"role":"user"|"assistant"|"system", "content":"..."}`
- `/chat` 响应：`steps[]`（每步 `capability` + `text`）、`summary`（汇总 Markdown）
- 错误：任一接口失败返回 `{"error":"..."}`（HTTP 状态码仍为 200，需检查 `error` 字段）
- 安全门：`safety` 步骤会给出红旗告警提示（建议立即就医）

## 3. 切换 / 启用真实 LLM

harness 的所有 Sub-Agent 统一走 LLM（无 rule/mock 切换开关）。配置优先级：
`resources/config.yaml` → 环境变量 `HARNESS_*` → 命令行参数。

### 3.1 用 LM Studio（最直接，无需权重/Docker）

LM Studio 加载 `google/gemma-4-12b-qat`（文本与视觉共用同一原生多模态端点），
开启本地服务器（默认 `http://localhost:11223/v1`），然后：

```powershell
# PowerShell（前缀是 HARNESS_，不是 TCM_）
$env:HARNESS_LLM_BASE_URL="http://localhost:11223/v1"
$env:HARNESS_LLM_API_KEY="<LM Studio → Developer → Server Settings 中的 API Key>"
$env:HARNESS_MODEL="google/gemma-4-12b-qat"
cd server/harness && ../target/debug/harness --listen 127.0.0.1:8011
```

等价于编辑 `server/harness/resources/config.yaml`：
```yaml
llm_base_url: "http://localhost:11223/v1"
llm_api_key: ""                  # LM Studio 开启校验时填写
model: "google/gemma-4-12b-qat"
llm_timeout_secs: 120
```

### 3.2 经 llm_server 网关（可选）

```powershell
cd llm_server && python -m app.main          # 网关 :8000
$env:HARNESS_LLM_BASE_URL="http://localhost:8000/v1"
```
经网关可使用 prompt 优化、tool calling、MCP 与 agent 循环等中间层能力，
详见 [`llm_server.md`](./llm_server.md)。

> **无 LLM 时**：`/health`、`/agents`、`/skills` 仍可用，`/chat` 与 `POST /agents` 会返回错误
> （harness 未提供 MockProvider）。确定性逻辑可离线验证：
> `cd server && cargo test -p harness --test cases`。
> 若 LM Studio 开启了 API Key 校验，必须填 `HARNESS_LLM_API_KEY`。

### 3.3 调整诊断流程顺序（不走 LLM 的纯编排改动）

编辑 `server/harness/resources/routing.yaml` 的 `active` 列表即可增删诊断步骤
（如去掉 `palpation` 跳过切诊），改完 `POST /reload` 生效：

```yaml
active: [inspection, listening, inquiry, palpation, differentiation, safety, treatment]
```

## 4. 常见问题

- **为什么有时中途直接结束？** 若为 `referred`，说明出现红旗症状，系统优先保障安全并引导线下就医。
- **为什么只问了很少几轮？** 当候选证候置信度达标（Top1≥0.55 且领先≥0.15）或接近兼证时即收敛，避免无谓打扰。
- **方案里出现西医检查？** 这是"更快更彻底痊愈"的主动设计：用西医手段明确诊断、排除器质病变，与中医方案互补。
- **孕期/备孕提示？** 方案阶段会追问，若选"孕期/备孕"则中药项会附加安全提示，用药须由专业医师辨证。
