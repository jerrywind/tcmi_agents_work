# llm_server v2：LM Studio 网关 + Agent 中间层

`llm_server/` 为系统提供「LLM 中间层」。**v2 起不再托管/内置任何模型**：
模型推理统一由宿主机 **LM Studio**（默认 `http://localhost:11223/v1`，模型
`google/gemma-4-12b-qat`）提供。本服务在其之上实现：

1. **prompt 优化** —— 上下文压缩、冗余合并、预算裁剪；
2. **tool calling** —— 工具注册表与执行循环；
3. **MCP** —— 以 MCP Client 接入外部 MCP Server，将其工具纳入 agent；
4. **agent** —— ReAct 风格多步工具调用循环。

同时暴露 OpenAI 兼容 API（`/v1/chat/completions`、`/v1/responses`、`/v1/embeddings`），
与 harness 的 LLM 调用完全兼容（OpenAI 兼容协议），**harness 无需改动**。

```
┌──────────┐   OpenAI 兼容    ┌─────────────────────┐   OpenAI 兼容   ┌──────────────────────┐
│  harness │ ──────────────▶ │      llm_server     │ ──────────────▶ │  LM Studio :11223    │
│  (Rust)  │                 │ 网关 + Agent 中间层   │                 │  google/gemma-4-12b  │
└──────────┘                 │                      │                 └──────────────────────┘
                             │  · prompt 优化       │
                             │  · tool calling      │
                             │  · MCP client        │──────────▶ 外部 MCP Server（可选）
                             │  · agent 循环         │
                             └─────────────────────┘
```

## 快速启动

### 前置：宿主机 LM Studio

1. LM Studio 加载 `google/gemma-4-12b-qat`（多模态，文本/视觉共用）；
2. Developer → Local Server 开启，默认端口 `11223`；
3. 记录 Server Settings 中的 API Key（关闭校验则任意非空值）。

> **模型事实（单一来源）**：当前默认 `routing.llm.yaml` 中文本与视觉（望诊）**共用同一个
> `google/gemma-4-12b-qat` 多模态端点**，不单独部署视觉模型。系统仍保留「经
> `TCM_LLM_VISION_BASE_URL` / `TCM_LLM_VISION_MODEL` 把视觉能力独立部署到专属端点」的
> 可选能力（非默认）。无上游时
> llm_server `/healthz` 返回 `degraded`、`/v1/models` 返回 503。
> harness **无 MockProvider**：LLM 不可用时 `/chat` 会返回错误，只读端点不受影响。

### 本地运行

```bash
cd llm_server
pip install -r requirements.txt
python -m app.main        # 监听 0.0.0.0:8000
```

### Docker 运行

```bash
cd llm_server
docker compose up --build
# 宿主机端口 22010 -> 容器 8000；容器经 host.docker.internal 访问宿主机 LM Studio
```

### harness 接入

| 场景 | `HARNESS_LLM_BASE_URL` |
|---|---|
| 经 llm_server 网关（本服务） | `http://localhost:8000/v1`（本地）／ `http://llm_server:8000/v1`（Docker 同网络） |
| 直连 LM Studio（无需中间层，默认值） | `http://localhost:11223/v1` |

配置方式：`resources/config.yaml` 的 `llm_base_url`，或环境变量 `HARNESS_LLM_BASE_URL`
（前缀是 `HARNESS_`，不是 `TCM_`）。模型用 `HARNESS_MODEL`，默认
`google/gemma-4-12b-qat`；视觉与文本共用同一端点。

## API 一览

| 端点 | 说明 |
|---|---|
| `GET /healthz` | 健康检查（含 LM Studio 连通性、工具数量） |
| `GET /v1/models` | 透传 LM Studio 模型列表 |
| `POST /v1/chat/completions` | OpenAI chat 兼容；带 `x-tcm-agent: 1` 或 `"agent": true` 时走网关内 agent 循环 |
| `POST /v1/responses` | 透传 LM Studio Responses API |
| `POST /v1/embeddings` | 透传 LM Studio（供 RAG 复用 embedding 模型） |
| `POST /v1/agent/run` | 完整 Agent 接口（prompt 优化 + MCP 工具 + tool calling 循环） |
| `GET /v1/agent/tools` | 查看当前可用工具（内置 + MCP） |

### Agent 接口示例

```json
POST /v1/agent/run
{
  "messages": [
    {"role": "system", "content": "你是中医助手，可调用工具获取信息。"},
    {"role": "user", "content": "现在几点？并算一下 123*45。"}
  ]
}
```

返回 `content`（最终答案）+ `trace`（每轮工具调用记录）+ `usage`（token 汇总）。

## 四个核心模块

| 模块 | 路径 | 说明 |
|---|---|---|
| prompt 优化 | `app/prompt/optimizer.py` | system 注入、相邻消息合并、超长截断、预算裁剪 |
| tool calling | `app/tools/` | `ToolRegistry` 统一管理 schema + handler；内置工具见 `builtin.py` |
| MCP | `app/mcp/` | 极简 Streamable HTTP Client（`initialize`/`tools/list`/`tools/call`），`MCP_CLIENTS` 声明外部 server |
| agent | `app/agent/loop.py` | ReAct 循环：调用模型 → 执行工具 → 回填结果 → 直到纯文本或达上限 |

### 接入 MCP Server

```bash
$env:MCP_CLIENTS = '[{"name":"kb","url":"http://127.0.0.1:9000/mcp","headers":{"Authorization":"Bearer sk-xxx"}}]'
```

工具以 `{name}_{tool}` 命名注入 agent 工具集；单个 server 连接失败不影响启动。

## 配置项

见 `llm_server/.env.example`。核心项：

| 变量 | 默认 | 说明 |
|---|---|---|
| `LMSTUDIO_BASE_URL` | `http://localhost:11223/v1` | LM Studio 端点（Docker 内用 `host.docker.internal`） |
| `LMSTUDIO_API_KEY` | `sk-noauth` | LM Studio API Key |
| `DEFAULT_MODEL` | `google/gemma-4-12b-qat` | 默认模型 id |
| `LLM_HOST` / `LLM_PORT` | `0.0.0.0` / `8000` | 监听地址 |
| `ENABLE_PROMPT_OPTIMIZE` | `true` | prompt 优化开关 |
| `AGENT_MAX_ROUNDS` | `8` | agent 最大轮数 |
| `ENABLE_MCP` / `MCP_CLIENTS` | `true` / `[]` | MCP 开关与客户端列表 |

## RAG（可选独立组件）

`llm_server/rag/` 保留为**可选独立组件**，不再随主服务自动启动。需要 RAG 时单独运行：

```bash
cd llm_server/rag
pip install numpy fastapi httpx
$env:RAG_EMBED_BASE_URL = "http://localhost:8000/v1"   # 走网关 /v1/embeddings 透传 LM Studio
python -m rag serve
```

> 前提：LM Studio 需额外加载一个 embedding 模型（如 `bge-m3`）以提供 `/v1/embeddings`；
> 图像 caption 需多模态模型（gemma-4-12b-qat 支持）。详见 [`rag.md`](./rag.md)。

## 常见问题

- **`/healthz` 显示 `degraded`**：LM Studio 未启动或未加载模型，业务请求返回 503；
  此时 harness 的 `/chat` 会返回错误（无 MockProvider 兜底），只读端点仍可用。
- **503 `upstream_unavailable`**：上游不可达时的统一错误码。
- **流式（`stream: true`）**：网关暂不转发流式；harness 默认非流式调用，不受影响。
- **prompt 优化是否影响响应质量**：只做无损/低损压缩（合并/截断/裁剪最旧历史），
  可用 `ENABLE_PROMPT_OPTIMIZE=false` 关闭。
