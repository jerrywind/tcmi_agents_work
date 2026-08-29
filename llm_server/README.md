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
| `rag/` | **可选独立组件**，需单独启动：`python -m rag serve` |

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

完整列表见 `.env.example` 与 [`docs/llm_server.md`](../docs/llm_server.md)。
