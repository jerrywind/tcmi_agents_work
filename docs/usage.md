# 使用文档（Usage）

面向两类使用者：**终端用户**（通过前端页面完成问诊）与**接入方**（通过 REST API 集成）。

> ⚠️ 免责声明：本系统由 AI 生成，仅供健康参考，**不构成医疗诊断或处方建议**。如有不适或红旗症状，请及时线下就医。

## 1. 产品使用流程（前端页面）

> **前端已切到 harness 契约**：旧的会话式 `services/api.ts` 已下线，6 个页面改用
> `services/harness.ts`，多轮 `messages` 由 `services/session.ts` 在前端维护。

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
   - **辨证结构**卡片（T4.1/T4.2）：主证与兼证并列展示，各带置信度、支持证据与矛盾证据
     （如「主证 脾胃湿热 80%｜支持：口苦、口臭…｜矛盾：（无）」），并给出基于主证的传变提示。
     兼证与主证是**并存**关系，不是备选方案。
   - 证候结论、辨证依据链（支持/矛盾证据）、调理建议、诊疗方案、免责声明，支持查看与分享。
   - **存证与回查**（T5.1）：显示报告编号（服务端启用归档时），
     「复制存证信息」把报告快照复制到剪贴板自行留存。
5. **存证记录**（`pages/reports`）
   - 列出服务端归档的报告（`GET /reports`），点开可回查完整快照（`GET /reports/:id`）。
   - 用途：换设备后找回上次结论，或纠纷时自证「当时给了什么建议」。
   - 服务端未启用持久化时列表为空并给出说明（不是错误）。
6. **技能管理**（`pages/skills`，可选）
   - 列出当前可用技能及其归属（`GET /skills`）：`tcm-kb`、`tcm-diet`、`tcm-rag` 等共 11 个，
     外加 `config.yaml` 里 `mcp_clients` 挂载的 `mcp__*` 外部工具。
   - harness 的内置技能为**编译期注册**，不支持运行时按名装载/卸载；
     新增技能需改 `src/skills/builtin.rs` 后重新构建；外部工具改配置即可。
   - 扩展知识库请优先改 `resources/*.yaml`，或用 `HARNESS_RAG_ENDPOINT` 接入自有 RAG。
     详见 [`SKILL 工具集`](./skills.md)。

## 2. REST API 接入（接入方）

Base URL（开发）：`http://localhost:8011`
（生产经 nginx 时为 `https://<域名>/api`，nginx 会剥离 `/api` 前缀后转发）

> **harness 是无状态服务**：不保存问诊会话，没有会话 id，也没有
> `start/answer/report/trace` 等会话端点；不提供图片上传与 `/uploads` 静态目录
> （图片以 base64 / URL 随请求传入）。
> 多轮问诊由**调用方**维护 `messages` 数组，每次带上完整对话历史。

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
#     "summary":"## 望诊\n...\n\n## 辨证\n...",
#     "failures":[], "partial":false,
#     "blocked":false, "skipped":[],
#     "structured":{"differentiation":{
#        "primary":{"slug":"spleen_stomach_damp_heat","name":"脾胃湿热",
#                   "confidence":0.8,
#                   "supporting":["口苦","口臭","肢体困重","脾胃湿热证据"],
#                   "conflicting":[], "pathogenesis":"湿热蕴结中焦…","score":4.0},
#        "concurrent":[{"slug":"liver_qi_stagnation","name":"肝郁气滞",
#                       "confidence":0.4, "supporting":["烦躁易怒","肝郁气滞证据"],
#                       "conflicting":[], "pathogenesis":"肝失疏泄…","score":2.0}],
#        "transformations":[]}},
#     "trace":[{"capability":"inspection","name":"望诊","duration_ms":1234,
#               "model":"...","llm_calls":2,"llm_attempts":2,
#               "prompt_tokens":..., "completion_tokens":..., "total_tokens":...,
#               "tool_calls":["tcm-vision"], "error":null}, ...]}
```
按 `resources/routing.yaml` 的 `active` 顺序依次调用各 Sub-Agent（望→闻→问→切→辨证→安全门→治疗），
返回每一步的输出 `steps` 与汇总文本 `summary`，并附逐步埋点 `trace`。

> **`/chat` 是一次性串行全跑，不是多轮对话**：一次请求把 `active` 列表里的
> 每个 Sub-Agent 各跑一遍（默认 7 步），直接返回全部结果。
> harness **没有**「问→等用户答→再问」的服务端循环，也**不会中途收敛**
> （唯一例外是安全门命中高危红旗时提前终止，见下）。
> 多轮交互必须由调用方实现：把历史问答累积进 `messages` 后再次 `POST /chat`。
>
> **部分失败降级**：某一步失败不再让整次 `/chat` 失败——已完成的步骤照常返回，
> 失败步骤记入 `failures` 并置 `partial: true`；只有**全部步骤都失败**
> （通常是 LLM 不可达）才返回 `{"error": ...}`。
>
> **安全门拦截（红旗）**：命中 `high`/`critical` 级红色警戒时，安全门之后
> 的步骤被**跳过**不再执行（默认即治疗步），响应返回 `blocked: true`、
> `block_reason` 与 `skipped[]`。调用方应据此引导就医，而不是展示治疗方案。
>
> **结构化输出（`structured`）**：目前只有辨证步产出，键为 `differentiation`，
> 含主证（`primary`）、兼证（`concurrent`）与传变提示（`transformations`）；
> 每个证候带 `confidence`（0~1）、`supporting[]`、`conflicting[]`、`pathogenesis`。
> 与正文 `steps[].text` 同源、同一份输入，但**不经过 LLM**，因此是确定性的：
> 调用方可直接按字段渲染，无需从 Markdown 反解析。
> 仅在该步骤**成功**时产出（步骤失败则 `structured` 里没有该 capability 的键，
> 此时看 `failures[]`）。详见 2.8。

`payload` 字段（全部可选，按需传）：

| 字段 | 类型 | 作用 |
|---|---|---|
| `syndrome` | string | 指定证候 slug/中文名，供治疗 Agent 检索方剂与调护 |
| `herbs` | string[] | 待校验的处方药味，供安全门与治疗 Agent 做配伍禁忌校验 |
| `pregnant` | bool | 妊娠禁忌校验开关，配合 `herbs` 使用 |
| 其它（如 `gender`/`age`/`region`） | any | 透传给各 Agent，当前版本 Agent 未读取 |

### 2.4 单步调用某个 Sub-Agent
```bash
curl -X POST http://localhost:8011/agents \
  -H 'Content-Type: application/json' \
  -d '{
    "capability": "differentiation",
    "messages": [{"role":"user","content":"口苦口臭、肢体困重、舌红苔黄腻"}],
    "payload": {}
  }'
# -> {"capability":"differentiation","content":"...","trace":{...},
#     "structured":{"primary":{...},"concurrent":[...],"transformations":[]}}
```
`capability` 取值：`inspection` | `listening` | `inquiry` | `palpation` |
`differentiation` | `safety` | `treatment`。

`structured` 仅辨证步有内容，其余步骤为 `null`（字段恒定存在，便于调用方无分支取值）。

### 2.5 技能（SKILL）
```bash
curl http://localhost:8011/skills                # 列出全部技能及归属（owner）
curl 'http://localhost:8011/skills?owner=treatment'   # 只看治疗步用得到的工具
curl -X POST http://localhost:8011/skills \
  -H 'Content-Type: application/json' \
  -d '{"name":"tcm-kb","arguments":{"query":"脾胃湿热"}}'
# -> {"result":{"name":"脾胃湿热","pathogenesis":"..."}}
```

**11 个内置技能及其入参**（详见 [`skills.md`](./skills.md)）：

| 技能 | 归属 | 入参 |
|---|---|---|
| `tcm-vision` | 望诊 | `{"text": "..."}` |
| `tcm-auscultation` | 闻诊 | `{"text": "..."}` |
| `tcm-inquiry` | 问诊 | `{"text": "..."}` |
| `tcm-palpation` | 切诊 | `{"text": "..."}` |
| `tcm-reference` | 辨证 | `{"text": "..."}` |
| `tcm-safety` | 安全门 | `{"text": "..."}` |
| `tcm-kb` | 全局 | `{"query": "..."}` |
| `tcm-diet` | 全局 | `{"syndrome": "..."}` |
| `tcm-rag` | 全局 | `{"query": "...", "top_k"?: number}` |
| `tcm-formula` | 治疗 | `{"syndrome": "..."}` |
| `tcm-care` | 治疗 | `{"syndrome": "..."}` |

> - 内置技能为**编译期注册**，不支持运行时装载/卸载；外部 MCP 工具改 `config.yaml` 的
>   `mcp_clients` 即可挂载。
> - 7 个 Sub-Agent 在推理时**会自动调用**自己可见的技能（每步最多 `max_tool_rounds` 轮），
>   调用轨迹见 `/chat` 响应 `trace[].tool_calls`；也可按上表显式 `POST /skills` 触发。
> - `POST /skills` 带 `owner` 时按该 capability 的可见范围过滤，越界调用返回
>   `{"error":"未知技能: xxx"}`。

### 2.6 热重载 YAML 资源
```bash
curl -X POST http://localhost:8011/reload   # -> {"ok":true}
```
需 `resources/config.yaml` 中 `hot_reload: true`。改完证候/方剂/问诊等 YAML 后调用即可，
无需重启（详见 [`deployment.md`](./deployment.md) 3.4）。
新增的 `contradictions.yaml` 同样走热重载；文件缺失时按空列表处理（只是没有矛盾证据）。

### 2.7 MCP 端点（供外部 MCP 客户端接入）

```bash
curl -X POST http://localhost:8011/mcp -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
# -> {"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"agent_inspection",...}, ...]}}
```

对外暴露 7 个 `agent_*` 工具（每个 capability 一个）+ `run_agent`（通用入口）+
`list_agent_capabilities`（能力清单，不需要 LLM）。
`tools/call` 等价于一次 `POST /agents`，辨证工具额外在 `structuredContent` 里
返回结构化的主证/兼证。完整工具表与错误约定见 [`mcp.md`](./mcp.md)。

### 2.8 报告归档与回查（T5.1，可选能力）

默认**关闭**：harness 保持无状态，不落任何盘。配置 `HARNESS_STORE_DIR`（或
`config.yaml` 的 `store_dir`）后，每次 `/chat` 落盘一份报告快照。

```bash
curl http://localhost:8011/reports          # -> {"reports":[{id,created_at,partial,blocked,steps,primary_syndrome}],"enabled":true}
curl http://localhost:8011/reports/20260830-101500-a1b2c3   # -> 完整快照（含 messages/payload/result）
```

- `/chat` 响应会多出 `report_id`（未启用时为 `null`）；
- 落盘内容**已脱敏**（手机号 / 身份证 / 邮箱 / 12 位以上数字串），
  与本次响应无关——用户看到的仍是原文；
- 未启用持久化时，`GET /reports` 返回 `{"reports": [], "enabled": false, "hint": "..."}`。
  这里用 `hint` 而不是 `error`：**未启用不是失败**，客户端统一把 `error` 当错误处理，
  用 `error` 会让调用方无法区分「功能没开」与「查不出来」。
- 报告 id 只含 `A-Za-z0-9_-`，服务端会拒绝含 `../` 之类的 id（防路径穿越）。

删除某份记录：直接删掉 `store_dir` 下对应的 `<id>.json` 即可（无索引文件）。

### 2.9 字段速览
- `Message`：`{"role":"user"|"assistant"|"system", "content":"..."}`
- `/chat` 响应：
  - `steps[]`（每步 `capability` + `text`）、`summary`（汇总 Markdown）
  - `disclaimer`：服务端下发的免责声明（**必须展示且不可被用户关闭**，T5.4）
  - `report_id`：归档报告 id（未启用持久化时为 `null`，见 2.8）
  - `failures[]`、`partial`：失败步骤与该次结果是否不完整
  - `blocked`、`blocked_by`、`block_reason`、`skipped[]`：安全门拦截标记与被跳过的步骤
  - `trace[]`：每步埋点（`capability`/`name`/`duration_ms`/`model`/`llm_calls`/
    `llm_attempts`/`llm_duration_ms`/`prompt_tokens`/`completion_tokens`/`total_tokens`/
    `tool_calls[]`/`error`）
  - `structured`：按 capability 键的结构化输出，目前只有 `differentiation`
    （`primary` / `concurrent[]` / `transformations[]`；每个证候含 `slug` / `name` /
    `confidence` / `supporting[]` / `conflicting[]` / `pathogenesis` / `score`）
- `POST /agents` 响应：`capability`、`content`、`trace`（同上，单步）、`structured`（无则为 `null`）
- 错误：任一接口失败返回 `{"error":"..."}`（HTTP 状态码仍为 200，需检查 `error` 字段）
- 安全门：命中 `high`/`critical` 红旗时**中断后续步骤**并置 `blocked: true`；
  `medium`（如妊娠）只告警不中断

## 3. 切换 / 启用真实 LLM

harness 的所有 Sub-Agent 统一走 LLM（无 rule/mock 切换开关）。配置优先级：
`resources/config.yaml` → 环境变量 `HARNESS_*` → 命令行参数。

### 3.1 直连 LM Studio（最直接，无需下载权重）

LM Studio 加载 `google/gemma-4-12b-qat`（文本与视觉共用同一原生多模态端点），
开启本地服务器（默认 `http://localhost:11223/v1`），然后：

```powershell
# 前缀是 HARNESS_，不是 TCM_
# 容器内访问宿主机用 host.docker.internal（写 localhost 会连到容器自己）
$env:HARNESS_LLM_BASE_URL = "http://host.docker.internal:11223/v1"
$env:HARNESS_LLM_API_KEY  = "<LM Studio → Developer → Server Settings 中的 API Key>"
$env:HARNESS_MODEL        = "google/gemma-4-12b-qat"

cd server
docker build -f harness/Dockerfile -t tcm-harness:local .
docker run -d --name tcm-harness-8011 -p 8011:8011 `
  -e HARNESS_LLM_BASE_URL -e HARNESS_LLM_API_KEY -e HARNESS_MODEL tcm-harness:local
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
> （harness 未提供 MockProvider）。确定性逻辑可在 Docker 内离线回归（见 [`testing.md`](./testing.md)）。
> 若 LM Studio 开启了 API Key 校验，必须填 `HARNESS_LLM_API_KEY`。

### 3.3 调整诊断流程顺序（不走 LLM 的纯编排改动）

编辑 `server/harness/resources/routing.yaml` 的 `active` 列表即可增删诊断步骤
（如去掉 `palpation` 跳过切诊），改完 `POST /reload` 生效：

```yaml
active: [inspection, listening, inquiry, palpation, differentiation, safety, treatment]
```

> **安全门不可被移除（T5.4 合规）**：`active` 里可以增删其它步骤（如去掉 `palpation`），
> 但如果**不含 `safety`**，harness 会在治疗步之前强制插入它并打 warn 日志。
> 允许把安全门从流程里删掉，等于让红旗症状绕过拦截直接走到治疗建议——
> 这是本系统最不能出错的一条路径，因此不接受配置关闭。

## 4. 常见问题

- **`/chat` 返回 `{"error": ...}`？** harness 的错误体**也用 HTTP 200** 返回，
  调用方必须检查 `error` 字段。最常见原因是 LLM 不可达（LM Studio 未启动或
  `HARNESS_LLM_BASE_URL` 配错）——此时**所有**步骤都失败。
  若只是个别步骤失败，响应会是 `partial: true` 且带 `failures[]`，已完成的步骤仍可用。
- **为什么一次 `/chat` 要等很久？** 默认会把 7 个 Sub-Agent 各调一次 LLM（串行）。
  减少步骤请改 `routing.yaml` 的 `active`（如只留 `differentiation`、`treatment`）。
- **为什么没有「问诊追问 → 用户回答 → 再追问」的循环？** harness 没有服务端会话与收敛逻辑。
  需要多轮时由调用方累积 `messages` 后重复 `POST /chat`。
- **红旗症状会被中断吗？** 会。命中 `high`/`critical` 级红色警戒时，安全门之后的步骤
  （默认即治疗步）直接跳过，响应返回 `blocked: true`、`block_reason` 与 `skipped[]`，
  调用方应据此引导就医。`medium` 级（如妊娠）只在输出里给出 `[severity] advice` 告警，
  不中断流程。
- **方案里出现西医检查？** 这是「更快更彻底痊愈」的主动设计：用西医手段明确诊断、
  排除器质病变，与中医方案互补。相关要求写在 `resources/prompts.yaml` 的 `treatment` 段。
- **孕期/备孕用药安全？** 调用时在 `payload` 传 `{"herbs": [...], "pregnant": true}`，
  安全门会做妊娠禁忌与十八反十九畏校验。
- **为什么 `GET /reports` 返回 `enabled: false`？** 报告持久化默认关闭，
  需配置 `HARNESS_STORE_DIR`。这是刻意设计：harness 默认无状态、不落盘（见 2.8）。
- **免责声明要不要自己写？** 不用，也不该自己写：服务端每份结果都带 `disclaimer` 字段，
  前端与第三方接入方应**优先展示服务端下发的版本**，避免各端文案漂移。
