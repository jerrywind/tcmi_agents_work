# llm_server v2 —— LM Studio 网关 + Agent 中间层

> 本服务**不再托管/内置任何模型**。模型推理统一由 **LM Studio**（默认
> `http://localhost:11223/v1`，模型 `google/gemma-4-12b-qat`）提供。
> llm_server 在其之上实现：**prompt 优化、tool calling、MCP、agent**，
> 并对 backend 保持 OpenAI 兼容（`/v1/chat/completions`、`/v1/responses`、
> `/v1/embeddings`），backend **无需改动**即可接入。

## 架构

```
┌──────────┐   OpenAI 兼容    ┌─────────────────────┐   OpenAI 兼容   ┌──────────────────────┐
│  backend │ ──────────────▶ │      llm_server     │ ──────────────▶ │  LM Studio :11223    │
│ (FastAPI)│                 │ 网关 + Agent 中间层   │                 │  google/gemma-4-12b  │
└──────────┘                 │                      │                 └──────────────────────┘
                             │  · prompt 优化       │
                             │  · tool calling      │
                             │  · MCP client        │──────────▶ 外部 MCP Server（可选）
                             │  · agent 循环         │
                             └─────────────────────┘
```

- **backend → llm_server**：`TCM_LLM_BASE_URL=http://localhost:8000/v1`（本地）或
  `http://llm_server:8000/v1`（Docker 同网络），`api: chat|responses` 均可。
- **llm_server → LM Studio**：默认 `http://localhost:11223/v1`；Docker 内自动使用
  `http://host.docker.internal:11223/v1`。

## 快速开始

### 本地（开发调试）

```powershell
cd llm_server
pip install -r requirements.txt
python -m app.main
# 服务监听 0.0.0.0:8000，/healthz 会显示 LM Studio 是否可达
```

### Docker

```powershell
cd llm_server
docker compose up --build
# 宿主机端口：22010（业务端口区间 22000-22200）
# 容器内自动经 host.docker.internal 访问宿主机 LM Studio
```

### backend 对接

本地：

```powershell
$env:TCM_LLM_BASE_URL = "http://localhost:8000/v1"
$env:TCM_LLM_API = "chat"          # 或 responses（网关均透传）
$env:TCM_LLM_TEXT_MODEL = "google/gemma-4-12b-qat"
```

Docker 联调（backend 与 llm_server 同一 compose 网络时）：

```powershell
$env:TCM_LLM_BASE_URL = "http://llm_server:8000/v1"
```

## API 一览

| 端点 | 说明 |
|---|---|
| `GET /healthz` | 健康检查（含 LM Studio 连通性与工具数量） |
| `GET /v1/models` | 透传 LM Studio 模型列表 |
| `POST /v1/chat/completions` | OpenAI chat 兼容；带 `x-tcm-agent: 1` 请求头或 `"agent": true` 时走网关内 agent 循环 |
| `POST /v1/responses` | 透传 LM Studio Responses API |
| `POST /v1/embeddings` | 透传 LM Studio（供 RAG 复用 embedding 模型） |
| `POST /v1/agent/run` | 完整 Agent 接口：prompt 优化 + MCP 工具 + tool calling 循环 |
| `GET /v1/agent/tools` | 查看当前可用工具（内置 + MCP） |

### Agent 接口示例

```json
POST /v1/agent/run
{
  "messages": [
    {"role": "system", "content": "你是中医助手，可调用工具获取信息。"},
    {"role": "user", "content": "现在几点？并算一下 123*45。"}
  ],
  "max_rounds": 6
}
```

返回：

```json
{
  "object": "agent.run",
  "model": "google/gemma-4-12b-qat",
  "content": "当前时间是……；123*45 = 5535。",
  "rounds": 2,
  "trace": [
    {"round": 1, "tool": "get_current_time", "arguments": {}, "output": "当前时间：…"},
    {"round": 2, "tool": "calculate", "arguments": {"expression": "123*45"}, "output": "计算 123*45 = 5535"}
  ],
  "usage": {"prompt_tokens": 120, "completion_tokens": 80, "total_tokens": 200}
}
```

## 四个核心能力

### 1. prompt 优化（`app/prompt/optimizer.py`）

对 `/v1/chat/completions` 在透传前做无损/低损压缩：

- 未提供 system 时注入默认 system 提示；
- 合并相邻同角色消息、剔除空消息；
- 超长单条「保首保尾」截断；总预算超限时丢弃最旧历史（保留 system 与最新 user）。

开关：`ENABLE_PROMPT_OPTIMIZE`、`PROMPT_MAX_CHARS`、`PROMPT_SYSTEM_BRIEF`。

### 2. tool calling（`app/tools/`）

- `ToolRegistry`：统一管理工具（JSON Schema 声明 + 可执行 handler）；
- 内置工具见 `app/tools/builtin.py`（时间/计算/骰子/回显/工具清单）；
- 业务工具可直接在 `app/runtime.py` 中 `register`，或经 MCP 接入。

### 3. MCP（`app/mcp/`）

- 极简 MCP **Client**（Streamable HTTP，2025-03-26 协议，无第三方 SDK 依赖）；
- 通过 `MCP_CLIENTS` 环境变量声明外部 MCP Server（JSON 数组）；
- 启动时自动 `initialize` + `tools/list`，把工具以 `{client}_{tool}` 注入 agent 工具集；
- 单个 server 连接失败不影响启动（降级为可用子集）。

```powershell
$env:MCP_CLIENTS = '[{"name":"kb","url":"http://127.0.0.1:9000/mcp","headers":{"Authorization":"Bearer sk-xxx"}}]'
```

### 4. agent（`app/agent/loop.py`）

ReAct 风格多步循环：调用模型 → 解析 `tool_calls` → 执行工具 → 回填 `tool` 消息 →
下一轮，直到模型给出纯文本或达到 `AGENT_MAX_ROUNDS`。返回最终文本 + 完整轨迹 + token 用量。

## 配置项

全部环境变量见 `.env.example`，核心项：

| 变量 | 默认 | 说明 |
|---|---|---|
| `LMSTUDIO_BASE_URL` | `http://localhost:11223/v1` | LM Studio 端点（Docker 内用 `host.docker.internal`） |
| `DEFAULT_MODEL` | `google/gemma-4-12b-qat` | 默认模型 id |
| `LLM_HOST` / `LLM_PORT` | `0.0.0.0` / `8000` | 监听地址 |
| `ENABLE_PROMPT_OPTIMIZE` | `true` | 是否启用 prompt 优化 |
| `AGENT_MAX_ROUNDS` | `8` | agent 最大轮数 |
| `ENABLE_MCP` / `MCP_CLIENTS` | `true` / `[]` | MCP 开关与客户端列表 |

## RAG（可选独立组件）

`rag/` 保留为**可选独立组件**（不在主服务中启动）。如需 RAG：

```powershell
cd llm_server/rag
$env:RAG_EMBED_BASE_URL = "http://localhost:8000/v1"   # 走网关 /v1/embeddings 透传 LM Studio
python -m rag serve                                    # 或按原 rag/README 启动
```

> 注意：LM Studio 需要额外加载一个 embedding 模型（如 `bge-m3`）才能提供
> `/v1/embeddings`；多模态 caption 需要加载视觉模型（gemma-4-12b-qat 支持）。

## 常见问题

- **`/healthz` 显示 `degraded`**：LM Studio 未启动或未加载模型。确认 LM Studio →
  Developer → Local Server 已开启，端口 11223。
- **`503 upstream_unavailable`**：上游不可达，业务请求会走 backend 的降级逻辑。
- **流式（`stream: true`）**：当前网关暂不转发流式，backend 默认非流式调用，不受影响。
