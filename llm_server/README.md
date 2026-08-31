# llm_server v2 —— LM Studio 网关 + Agent 中间层

> **完整文档见 [`docs/llm_server.md`](../docs/llm_server.md)**；
> RAG 子组件见 [`docs/rag.md`](../docs/rag.md)。本文件只保留组件速查信息。

本服务**不托管任何模型**，模型推理统一由宿主机 **LM Studio**（默认
`http://localhost:11223/v1`，模型 `google/gemma-4-12b-qat`）提供。
llm_server 在其之上实现：**prompt 优化、tool calling、MCP、agent 循环**，
并对调用方保持 OpenAI 兼容。

```
harness (Rust) ──OpenAI 兼容──▶ llm_server :8000 ──OpenAI 兼容──▶ LM Studio :11223
                                 · prompt 优化
                                 · tool calling ──▶ 外部 MCP Server（可选）
                                 · agent 循环
```

## 启动

```powershell
# 本地
cd llm_server
pip install -r requirements.txt
python -m app.main                 # 监听 0.0.0.0:8000

# Docker
docker compose up --build          # 宿主机 22010 -> 容器 8000
```

## harness 接入

```powershell
$env:HARNESS_LLM_BASE_URL = "http://localhost:8000/v1"   # 本地经网关
$env:HARNESS_LLM_BASE_URL = "http://llm_server:8000/v1"  # Docker 同网络
# 或直连 LM Studio（harness 默认值）：http://localhost:11223/v1
```

> 环境变量前缀是 **`HARNESS_`**（不是 `TCM_LLM_*`，那是已废弃的旧写法）。

## 目录

| 路径 | 说明 |
|---|---|
| `app/gateway.py` | OpenAI 兼容网关（`/v1/chat/completions`、`/v1/responses`、`/v1/embeddings`） |
| `app/prompt/optimizer.py` | prompt 优化：system 注入、相邻消息合并、超长截断、预算裁剪 |
| `app/tools/` | `ToolRegistry` + 内置工具 |
| `app/mcp/` | MCP Client（Streamable HTTP，无第三方 SDK） |
| `app/agent/loop.py` | ReAct 风格多步工具调用循环 |
| `app/rrclient.py` | 向 rrserver 主动注册（换取 hash code）+ 周期心跳 + 优雅注销 |
| `rag/` | **可选独立组件**，需单独启动：`python -m rag serve` |

## rrserver 注册与心跳（可选）

配置 `RR_SERVER_BASE` 后启用：启动时 `POST /api/register` 换取独立 **hash code**，
后台每 **30 分钟** `POST /api/heartbeat` 上报存活；关机时主动注销。
云端 40 分钟没收到心跳会访问 `GET /rr/heartbeat` 探活，探活失败则注销注册
（本服务下次心跳收到 404 会自动重新注册）。详见
[`server/rrserver/README.md`](../server/rrserver/README.md#注册--心跳--探活)。

## 常用环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `LMSTUDIO_BASE_URL` | `http://localhost:11223/v1` | LM Studio 端点（Docker 内用 `host.docker.internal`） |
| `LMSTUDIO_API_KEY` | `sk-noauth` | LM Studio API Key |
| `DEFAULT_MODEL` | `google/gemma-4-12b-qat` | 默认模型 id |
| `LLM_HOST` / `LLM_PORT` | `0.0.0.0` / `8000` | 监听地址 |
| `ENABLE_PROMPT_OPTIMIZE` | `true` | prompt 优化开关 |
| `AGENT_MAX_ROUNDS` | `8` | agent 最大轮数 |
| `ENABLE_MCP` / `MCP_CLIENTS` | `true` / `[]` | MCP 开关与客户端列表 |
| `RR_SERVER_BASE` | 空 | rrserver 基址；留空则不注册 |
| `RR_SERVICE_NAME` / `RR_SERVICE_TOKEN` | `llm-server` / 空 | 注册凭据（须与 rrserver `[[tunnels]]` 一致） |
| `RR_SERVICE_ENDPOINT` | 空 | 本服务可被 rrserver 直达的基址（留空按 ws 隧道注册） |

> 心跳周期不在本侧配置：由 rrserver 的 `[health] heartbeat_interval_secs` 统一下发（默认 30 分钟）。

完整列表见 `.env.example` 与 [`docs/llm_server.md`](../docs/llm_server.md)。

> ⚠️ 真实 Key 只写进 `.env`（已被 `.gitignore` 忽略），
> **不要写进 `.env.example`**——它不受忽略规则保护，会被提交进仓库。
