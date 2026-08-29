# MCP 集成（现状与实施方案）

MCP（Model Context Protocol）在本项目有两个方向：

- **MCP Client**：把外部 MCP Server 的工具接进来，供 Sub-Agent 调用；
- **MCP Server**：把本系统的 7 个中医能力暴露给外部 MCP 客户端（Claude Desktop / Cursor 等）。

> ## 现状一句话（2026-08-29 更新）
> **两个方向都通了**：
> - **Client**：`config.yaml` 声明 `mcp_clients` 即可把外部 MCP Server 的工具
>   挂成 `mcp__<client>__<tool>` 技能，供全部 Sub-Agent 调用；
> - **Server**：`POST /mcp`（`src/mcp/server.rs`）对外暴露 7 个 `agent_*` 工具，
>   外部 MCP 客户端（Claude Desktop / Cursor 等）可直接调用。
>
> 若只是自己集成，用 REST 端点（`/chat`、`/agents`、`/skills`）更直接，
> 见 [`usage.md`](./usage.md)；要接入标准 MCP 客户端才走 `/mcp`。

---

## 1. 已实现的部分（库）

Client 在 `server/harness/src/mcp/mod.rs`，Server 在 `server/harness/src/mcp/server.rs`；
传输同为 **Streamable HTTP**，JSON-RPC 2.0 over `POST`，无第三方 SDK 依赖。

| 函数 | 方法 | 超时 | 说明 |
|---|---|---|---|
| `call_tool(client, url, tool, args)` | `tools/call` | 60s | 调用远端工具，返回 `result` 字段 |
| `list_tools(client, url)` | `tools/list` | 30s | 列出远端工具 |

请求头固定带 `Content-Type: application/json` 与 `Accept: application/json, text/event-stream`；
响应兼容 **SSE**（剥掉 `data:` 前缀）与**纯 JSON** 两种形态。

### 1.1 已接线：Client 挂载链路

`src/skills/builtin.rs::mount_mcp_clients` 在 `AppState::load` 中被调用；
`src/skills/toolcall.rs` 提供技能构造器：

- `mount_mcp_clients(reg, cfg, client)` —— 按 `config.yaml` 的 `mcp_clients` 批量挂载；
- `mcp_skill_named(name, remote_tool, desc, params, url, client)` —— 显示名与远端工具名不同时使用
  （挂载链路用这个：`mcp__kb__search_kb` 显示，远端仍用 `search_kb` 调用）；
- `mcp_skill(...)` / `mount_mcp(...)` / `http_skill(...)` —— 供手工注册单个工具。

---

## 2. Client 用法（已实现）

### 2.1 配置

在 `resources/config.yaml` 增加（也可用环境变量 `HARNESS_MCP_CLIENTS`
，格式 `name=kb,url=http://...;name=emr,url=http://...`）：

```yaml
mcp_clients:                       # 启动时连接，逐个注册为 Skill
  - name: kb                       # 工具以 mcp__kb__<tool> 注册
    url: http://localhost:9001/mcp
    tools: []                      # 白名单；留空 = tools/list 全部注册
    enabled: true                  # false 可临时停用而不删配置
```

挂载后即为**全局技能**（`owner` 为空，对所有 capability 可见），
`GET /skills` 可见、LLM tool calling 与内置技能完全一致，对编排器零侵入。

- 连接失败：打日志并跳过，**不阻断启动**（与 `llm_server` 的 MCP 策略一致）；
- 调用失败：`mcp_skill` 返回 `Err`，被 `chat_with_tools` 捕获为 `{"error": ...}` 回填，不击穿问诊流程；
- 验证：`curl -X POST localhost:8011/skills -d '{"name":"mcp__kb__search_kb","arguments":{...}}'`。

---

## 3. Server 用法（已实现，T4.5）

端点：`POST http://localhost:8011/mcp`（生产经 nginx 时为 `https://<域名>/api/mcp`）。
传输为 **Streamable HTTP**，JSON-RPC 2.0，与 client 侧同构、无第三方 SDK。

| 方法 | 说明 |
|---|---|
| `initialize` | 返回 `protocolVersion` / `capabilities` / `serverInfo` |
| `ping` | 空响应，健康检查 |
| `tools/list` | 返回 9 个工具定义 |
| `tools/call` | 执行工具；无 `id` 的通知（`notifications/*`）不回包（204） |

### 3.1 暴露的工具

| 工具名 | capability | 入参 | 产出 |
|---|---|---|---|
| `agent_inspection` | `inspection` | `messages` | 望诊结论 |
| `agent_listening` | `listening` | `messages` | 闻诊证据 |
| `agent_inquiry` | `inquiry` | `messages` | 下一个追问 |
| `agent_palpation` | `palpation` | `messages` | 切诊证据 |
| `agent_differentiation` | `differentiation` | `messages` | 主证/兼证与置信度（含 `structuredContent`） |
| `agent_safety` | `safety` | `messages` | 红旗告警 |
| `agent_treatment` | `treatment` | `messages` | 诊疗方案 |
| `run_agent` | — | `{capability, messages, payload}` | 通用入口（capability 接受 slug 或中文名） |
| `list_agent_capabilities` | — | — | 能力清单（纯查表，不需要 LLM） |

无会话状态：每次调用自带完整 `messages`，调用方自行维护多轮。

### 3.2 调用示例

```bash
# 1) 列出工具
curl -X POST http://localhost:8011/mcp -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'

# 2) 调辨证（需要真实 LLM）
curl -X POST http://localhost:8011/mcp -H 'Content-Type: application/json' \
  -d '{
    "jsonrpc":"2.0","id":2,"method":"tools/call",
    "params":{
      "name":"agent_differentiation",
      "arguments":{"messages":[{"role":"user","content":"口苦口臭、肢体困重、烦躁易怒"}]}
    }
  }'
# -> {"jsonrpc":"2.0","id":2,"result":{
#      "content":[{"type":"text","text":"【结构化辨证】..."}],
#      "structuredContent":{"primary":{...},"concurrent":[...],"transformations":[...]},
#      "isError":false}}

# 3) 查能力清单（不需要 LLM，可用来探活）
curl -X POST http://localhost:8011/mcp -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call",
       "params":{"name":"list_agent_capabilities","arguments":{}}}'
```

### 3.3 错误约定

分两层，**不要混为一谈**：

| 层 | 场景 | 返回 |
|---|---|---|
| 协议层 | 工具名不存在、`capability` 非法、缺 `messages` | JSON-RPC `error`（`-32601` / `-32602`） |
| 执行层 | Sub-Agent 跑挂了（多半是 LLM 不可达） | 正常 `result` + `isError: true`，文本为失败原因 |

执行层失败之所以走 `isError` 而非 JSON-RPC error，是为了让**模型**看得到失败原因
并自行决定下一步（换个能力、或告诉用户重试），而不是拿到一条干瘪的错误码。

### 3.4 与 REST 端点怎么选

- 自己集成、要完整流程（`steps` + `trace` + 降级/拦截语义）→ 用 `POST /chat`；
- 要接标准 MCP 客户端（Claude Desktop / Cursor）→ 用 `/mcp`；
- `/mcp` 的 `tools/call` 本质上就是一次 `run_single`，两者共用编排器、技能与埋点，
  能力上等价，差别只在协议外壳与返回结构。

> **未做**：stdio 传输。当前只有 Streamable HTTP；若需本地 stdio 接入再加。

---

## 4. 相关任务

见 [`tasks.md`](./tasks.md)：T2.4（MCP Client 接线，**已完成**）、
T4.5（MCP Server，**已完成**）。
