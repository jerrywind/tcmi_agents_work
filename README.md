# 中医智能问诊 Agent（望闻问切多 Sub-Agent 架构）

- `backend/`：Python3 + FastAPI，主诊编排器驱动「望闻问切 → 辨证 → 提问/出报告」诊断 Loop，
  六个 Sub-Agent（望/闻/问/切/辨证/安全）加一个诊疗方案 Sub-Agent，通过统一协议注册，
  可按 `routing.yaml` 一行配置切换实现/模型。诊断完成后自动进入**诊疗方案阶段**：结合
  用户个体情况（煎药便利性、是否接受外治、是否愿做西医检查、孕期备孕等）追问 1~2 条后，
  产出以"更快、更彻底痊愈"为目标的综合方案——不止开中药，还包括针灸推拿、外治法、
  西医检查（明确诊断/排除器质病变）、生活调护等多模态方案。
- `frontend/`：Taro 4 + React（H5 / 微信小程序多端），档案创建 → 问诊对话 → 诊断报告。
- `llm_server/`：可部署的本地大模型服务（基于 llama.cpp，默认 **文本 qwen3.6-9B + 视觉 Qwen3-VL**），内置 Python3 实现的 **RAG 服务**（文本/图像/图文对应检索，见 `docs/rag.md`），
  通过 OpenAI 兼容 API 为各 Sub-Agent 提供 LLM「大脑」（见 `docs/llm_server.md`）。
- `docs/agent-protocol.md`：Sub-Agent 可替换协议规范。
- `docs/sub_agents.md`：各 Sub-Agent 职责、system prompt 设计、专属技能映射。
- `docs/skills.md`：SKILL 工具集（registry/loader/toolcall + 8 个内置技能 + 热装载）。
- `docs/mcp.md`：MCP 集成（两层工具粒度的 MCP Server、MCP Client、能力远程化 `impl: mcp`）。
- `docs/plan.md`：项目计划与里程碑路线图。
- `docs/README.md`：文档中心入口（开发/部署/使用/协议）。
- `docs/development.md`：架构、目录、如何新增/切换 Sub-Agent、测试。
- `docs/deployment.md`：前后端部署、环境变量、Docker、Nginx 反代、生产注意。
- `docs/usage.md`：产品流程、REST API 调用示例、切换真实 LLM、FAQ。
- `docs/cleanup-rules.md`：垃圾文件命名约定与清理脚本 `scripts/cleanup.ps1` 规范（避免误删、避免污染）。
- `docs/testing.md`：测试结构与运行方式（含临时文件清理约束）。

## 后端启动

本项目有三个后端进程，可在本地分别启动；未配置 LLM 时 backend 自动回退 `MockProvider`，整套系统可离线运行。

### 1) backend（主诊编排 API，端口 8000）

```bash
cd backend
pip install -r requirements.txt
uvicorn app.main:app --reload --host 127.0.0.1 --port 8000
```

- 默认使用**内存存储**（`MemoryStore`），无需 Redis 即可运行；多 worker / 多实例需共享会话时设
  `TCM_STORE=redis` 与 `TCM_REDIS_URL`（见 `backend/app/store.py`，`RedisStore` 与 `MemoryStore` 接口一致）。
- 冒烟测试：`python smoke_test.py`（纯规则链路，模拟完整问诊收敛到"脾胃湿热"）
- 接入 LLM：设置环境变量 `TCM_LLM_API_KEY` 与 `TCM_LLM_BASE_URL`（文本端点）、
  `TCM_LLM_VISION_BASE_URL`（视觉端点），并把对应 capability 的 `impl` 切到 `llm` / `llm_vision`
  （最简方式：`TCM_ROUTING_FILE` 指向 `routing.llm.yaml`）。调用协议由 `TCM_LLM_API` 控制：
  `responses`（LM Studio Responses API，默认）或 `chat`（传统 Chat Completions）。
- 查看路由：`GET /api/system/agents`；调用轨迹：`GET /api/consultations/{id}/trace`

### 2) LLM 后端（文本 + 视觉大模型，二选一）

backend 通过 OpenAI 兼容协议调用大模型，承担听/问/切/辨证/安全/施治（文本）与望诊（视觉）。
启用真实 LLM：设 `TCM_LLM_BASE_URL` / `TCM_LLM_API_KEY` / `TCM_LLM_TEXT_MODEL` /
`TCM_LLM_VISION_MODEL`，并把 `TCM_ROUTING_FILE` 指向 `app/routing.llm.yaml`
（各 Sub-Agent 切换为 `llm` / `llm_vision` 实现）。未配置时自动回退 `MockProvider` 离线运行。

- **方案 A（推荐本地开发）：LM Studio 等本地推理服务**
  在 LM Studio 加载任意多模态模型（如 `google/gemma-4-12b-qat`，文本/视觉共用同一端点），
  开启本地服务器（默认 `http://localhost:11223/v1`），并设置：

  ```bash
  $env:TCM_LLM_BASE_URL="http://localhost:11223/v1"
  $env:TCM_LLM_API_KEY="<LM Studio → Developer → Server Settings 中的 API Key>"
  $env:TCM_LLM_TEXT_MODEL="google/gemma-4-12b-qat"
  $env:TCM_LLM_VISION_MODEL="google/gemma-4-12b-qat"   # 视觉与文本共用同一端点
  $env:TCM_LLM_API="responses"                          # 使用 LM Studio Responses API（默认）
  $env:TCM_ROUTING_FILE="app/routing.llm.yaml"
  ```

  > `routing.llm.yaml` 默认 `api: responses`；如需传统 Chat Completions 设 `TCM_LLM_API=chat`。
  > 若 LM Studio 开启 API Key 校验，填入 Developer → Server Settings → API Key 的值；关闭校验则任意非空值均可。
  > 仓库已提供 `backend/start_backend.ps1` 一键以该配置启动。

- **方案 B（自建）：`llm_server`（llama.cpp，需 GGUF 权重）**，详见 `llm_server/README.md`：

  ```bash
  # 需先准备 GGUF 权重到 llm_server/models/（文本 qwen3.6-9B + 视觉 Qwen3-VL）
  cd llm_server && docker compose --profile vision up --build
  ```

### 3) rrserver（家庭 LLM 反向隧道中继，端口 8080）

把无公网 IP 的家庭内 `llm_server` 经主动 WebSocket 隧道注册到云端，供 backend 经 `TCM_LLM_BASE_URL`
访问，详见 `rrserver/README.md`。

```bash
cd rrserver
cargo build --release --locked
cp config/rrserver.toml.example config/rrserver.toml   # 改 external_ws_base / token 为真实值
./target/release/rrserver server --listen 127.0.0.1:8080 --config config/rrserver.toml
```

## 前端启动

```bash
cd frontend
npm install
npm run dev:h5      # H5，devServer 已代理 /api -> 127.0.0.1:8000
npm run dev:weapp   # 微信小程序（用开发者工具打开 frontend 目录）
```

## 核心 API

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | /api/consultations | 新建问诊档案（基本信息+自述+自测项） |
| POST | /api/consultations/{id}/images | 上传舌象/面相/患处照片 |
| POST | /api/consultations/{id}/start | 启动诊断 Loop |
| POST | /api/consultations/{id}/answer | 回答问题，推进 Loop |
| GET | /api/consultations/{id} | 会话状态（问题/候选证候/消息流/报告） |
| GET | /api/consultations/{id}/report | 诊断报告 |
| GET | /api/skills | 列出已装载的 SKILL 技能与工具 |
| POST | /api/skills/load | 装载技能（按名或路径） |
| POST | /api/skills/unload | 卸载技能 |
| GET | /api/mcp/status | MCP 总览：Server 挂载状态、外部连接、各能力实现 |
| GET | /api/mcp/tools | 本 MCP Server 暴露的全部工具及 schema |
| POST | /api/mcp/clients | 运行时连接外部 MCP Server |
| DELETE | /api/mcp/clients/{name} | 断开外部 MCP Server 并卸载其工具 |
| POST | /api/families | 创建家庭（自动含「本人」成员） |
| GET | /api/families | 列出家庭 |
| GET | /api/families/{fid} | 家庭详情（含成员） |
| POST | /api/families/{fid}/members | 添加成员（体质档案+备注） |
| PATCH | /api/families/{fid}/members/{mid} | 更新成员档案 |
| GET | /api/families/{fid}/consultations?member_id= | 按家庭/成员列出全部问诊档案 |
| POST | /api/consultations/{cid}/ppg | 上传 PPG 采样序列（或 `simulate` 触发模拟波形）解析为脉象证据（source=切） |

## 脉象能力（PPG 硬件接入）

`backend/app/knowledge/ppg.py` 提供纯算法、无外部依赖的 PPG 解析：

- `synthesize_ppg(...)`：生成贴近真实指脉氧波形的模拟信号（normal/滑/涩/弱/弦），用于无硬件演示与联调。
- `analyze_ppg(samples, fs)`：峰值检测估算脉率与节律，并由波形幅值/灌注/上升斜率推断脉位（浮/中/沉）、脉力（有力/无力/和缓）、脉形（滑/涩/平）。
- 解析结果存入会话 `ppg` 字段，并以高置信度（`source=切`）汇入证据池；切诊 Sub-Agent 优先采用，未接入时降级为自测心率（低置信度）。

硬件接入：将真实采样序列（归一化浮点）POST 到 `/api/consultations/{cid}/ppg`（`samples`+`fs`）即可，无需改动上层。仅作健康参考，不替代专业诊疗。

## MCP 支持（既做 Server 又做 Client）

本项目对 MCP（Model Context Protocol）做了双向支持，依赖 `mcp==1.9.4`。
完整设计与踩坑记录见 [`docs/mcp.md`](docs/mcp.md)。

### 1) 作为 MCP Server（暴露中医能力）

对外提供**两层粒度**共 19 个工具：

- **会话级**（10 个）：`create_consultation` / `upload_image` / `upload_ppg` /
  `start_consultation` / `answer_question` / `get_state` / `get_report` /
  `list_families` / `create_family` / `add_member`
  —— 面向"帮我完整跑一次问诊"的对话式客户端。
- **Agent 级**（9 个）：`agent_inspection`（望）/ `agent_listening`（闻）/
  `agent_inquiry`（问）/ `agent_palpation`（切）/ `agent_differentiation`（辨证）/
  `agent_treatment`（治法）/ `agent_safety`（安全），外加 `run_agent` 通用入口与
  `list_agent_capabilities` 自省
  —— **无状态**、原子化，外部可只借用其中某一项中医能力。

两种传输：

- **Streamable HTTP**：随后端启动自动挂载在 `/mcp`（由 `routing.yaml` 的
  `mcp.server` 控制），客户端连 `http://<host>:8000/mcp`。
- **stdio**（Claude Desktop / Cursor 本地接入）：
  ```bash
  cd backend && python -m app.mcp.server
  ```
- 也可独立部署（适合把望诊等重负载能力单独放 GPU 机器）：
  ```bash
  python scripts/run_mcp_http.py    # http://0.0.0.0:8001/mcp
  ```

### 2) 作为 MCP Client（调用外部 MCP 工具）

`MCPToolHub` 可连接外部 MCP Server（`http` / `sse` / `stdio`），把其工具以
`mcp__<server>__<tool>` 注册进本系统 `skill_registry`，LLM 在望闻问切/辨证/施治
推理时即可通过 function calling 调用。

连接方式：写进 `routing.yaml` 的 `mcp.clients`（随后端启动自动连接），
或运行时调用 `POST /api/mcp/clients`；离线联调可用 `scripts/run_mcp_client.py`。

### 3) 把某项能力整体远程化

由于 Sub-Agent 协议是**无状态 + JSON 信封**，任一能力都可路由到远程 MCP Server，
编排器代码零改动：

```yaml
routing:
  diagnosis.inspection:
    impl: mcp                  # 由本地实现切换为远程
    options:
      server: vision_farm      # mcp.clients 中的连接名
```

远端不可用/超时会自动降级为 `status=error` 信封，不中断问诊。

## 免责声明

本项目输出仅供健康参考，不构成医疗诊断或处方建议；红旗症状会中断问诊并引导就医。
