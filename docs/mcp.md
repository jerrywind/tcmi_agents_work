# MCP 集成设计（Spec & Plan）

本项目同时扮演 **MCP Server**（把中医诊疗能力暴露给外部）与 **MCP Client**
（把外部 MCP 工具接入本系统的 Sub-Agent / SKILL 体系）。

本文是 MCP 能力的设计规格与实施计划，配套实现见 `backend/app/mcp/`。

---

## 1. 背景与问题

改造前 `app/mcp/` 只有两个孤立模块，存在以下问题：

| # | 问题 | 说明 |
|---|------|------|
| P1 | Server 未挂载 | `/mcp` 仅存在于注释中，`main.py` 没有真正挂载，远端客户端无法访问 |
| P2 | 粒度单一 | 只暴露"整段问诊流程"（create/start/answer），外部无法单独复用**望闻问切**等原子能力 |
| P3 | Client 无生命周期 | `MCPToolHub` 无人实例化，没有配置化连接、没有随应用启动/关闭 |
| P4 | 无法远程化 Sub-Agent | `SubAgent` 协议本身是无状态 + JSON 信封，天然可远程，但缺少 `impl="mcp"` 的桥接实现 |
| P5 | 传输不匹配 | Client 的 `connect_http` 用 SSE，而 Server 侧推荐 Streamable HTTP，二者对不上 |
| P6 | 无配置/无文档/无测试 | 硬编码，无 `routing.yaml` 配置项，零测试覆盖 |

## 2. 设计目标

- **G1 能力原子化**：7 个 capability（望/闻/问/切/辨证/治法/安全）各自成为一个 MCP 工具，可被外部单独调用。
- **G2 双向对称**：本地 Sub-Agent 与远程 MCP Sub-Agent 对编排器完全等价，切换只改 `routing.yaml` 一行。
- **G3 零侵入**：不改动 `orchestrator` 的调用逻辑，通过既有 `register` / `resolve` 扩展点接入。
- **G4 可运维**：配置化连接、启动自动接入、失败降级、运行时可查可管。

## 3. 架构

```
                    ┌─────────────────────────────────────┐
  外部 MCP Client   │          本项目 (backend)            │
  (Claude/Cursor)   │                                     │
        │           │   ┌──────────────┐                  │
        │  stdio    │   │ orchestrator │                  │
        └──────────>│   └──────┬───────┘                  │
        │           │          │ resolve(capability)      │
        │  HTTP     │   ┌──────▼───────┐                  │
        └──────────>│   │  protocol    │                  │
         /mcp       │   │  registry    │                  │
                    │   └──┬────────┬──┘                  │
   ┌────────────────┤      │        │                     │
   │  MCP Server    │  本地 impl   impl="mcp"             │
   │  (agent tools) │  (rule/llm)   │                     │
   └────────────────┤               ▼                     │
                    │      ┌─────────────────┐            │
                    │      │ McpRemoteAgent  │            │
                    │      └────────┬────────┘            │
                    │               │ MCPToolHub          │
                    └───────────────┼─────────────────────┘
                                    ▼
                          外部 MCP Server（远程 sub-agent / 第三方工具）
```

### 3.1 Server 侧：两层工具粒度

**A. 会话级工具（粗粒度，已有，保留）** —— 面向"帮我做一次完整问诊"的对话式客户端：

`create_consultation` / `upload_image` / `upload_ppg` / `start_consultation` /
`answer_question` / `get_state` / `get_report` / `list_families` / `create_family` / `add_member`

**B. Agent 级工具（细粒度，新增）** —— 面向"只借用某项中医能力"的调用方，
每个 capability 一个工具，直接走 `AgentRequest/AgentResponse` 信封：

| 工具名 | capability | 输入要点 |
|--------|-----------|---------|
| `agent_inspection` | `diagnosis.inspection` | `images`（含 type/path）|
| `agent_listening` | `diagnosis.listening` | `audio` / 文本描述 |
| `agent_inquiry` | `diagnosis.inquiry` | `evidences` + `asked_keys`，产出下一个问题 |
| `agent_palpation` | `diagnosis.palpation` | `ppg` / 脉象特征 |
| `agent_differentiation` | `diagnosis.differentiation` | `evidences`，产出 `hypotheses` |
| `agent_treatment` | `treatment.plan` | `hypotheses`，产出 `plans` |
| `agent_safety` | `diagnosis.safety` | 文本/证据，产出 `alerts` |

统一附加两个自省工具：

- `list_agent_capabilities`：列出全部 capability、当前路由 impl、可用实现列表。
- `run_agent`：通用入口，`{capability, payload, evidences, ...}`，等价于直接投递信封。

> 设计要点：Agent 级工具**无会话状态**，输入即完整上下文，与 `SubAgent` 无状态约定一致，
> 因此可安全并发调用、可水平扩展。

### 3.2 Client 侧：远程 Sub-Agent 桥

新增 `McpRemoteAgent`（`impl_name = "mcp"`），为 7 个 capability 各注册一个子类。
它把 `AgentRequest` 序列化为 MCP `call_tool` 参数，把返回的 JSON 反序列化回 `AgentResponse`。

`routing.yaml` 里改一行即可把某能力远程化：

```yaml
routing:
  diagnosis.inspection:
    impl: mcp              # 由本地 llm 切换为远程 MCP
    options:
      server: vision_farm  # mcp.clients 中的连接名
      tool: agent_inspection   # 可选，默认按 capability 推断
```

请求信封的序列化约定：`evidences` / `hypotheses` / `asked_keys` / `payload`
原样传递，同时把 `payload` 的键**铺平**到顶层参数，因此远端既可以按
`payload` 对象接收，也可以按扁平字段声明 schema。若 `tool` 指定为
`run_agent`，还会自动带上 `capability` 字段。

失败策略：远程不可用 / 超时 → 返回 `status=error` 信封，编排器沿用既有降级路径
（`_call` 中已有 degraded 逻辑），不会中断问诊。

### 3.3 配置

`routing.yaml` 新增 `mcp` 段：

```yaml
mcp:
  server:
    enabled: true                # 是否在 FastAPI 挂载 MCP 端点
    mount_path: /mcp             # Streamable HTTP 挂载路径
    expose_session_tools: true   # 会话级工具
    expose_agent_tools: true     # Agent 级工具
  clients:                       # 启动时自动连接的外部 MCP Server
    - name: vision_farm
      transport: http            # http | sse | stdio
      url: http://localhost:9001/mcp
      enabled: false
    - name: local_tools
      transport: stdio
      command: python
      args: ["-m", "some_mcp_server"]
      enabled: false
  call_timeout: 30               # 单次远程调用超时（秒）
```

环境变量覆盖（优先级高于 YAML）：

| 变量 | 说明 |
|------|------|
| `TCM_MCP_SERVER_ENABLED` | `0/false/no` 关闭 `/mcp` 挂载 |
| `TCM_MCP_MOUNT_PATH` | 修改挂载路径（自动补前导 `/`）|
| `TCM_MCP_CALL_TIMEOUT` | 远程调用超时秒数 |
| `TCM_MCP_CLIENTS` | JSON 数组，整体覆盖 `clients` |

### 3.4 管理端点

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/mcp/status` | Server 挂载状态 + 已连接客户端 + 工具数 |
| GET | `/api/mcp/tools` | 本 Server 暴露的全部 MCP 工具清单 |
| POST | `/api/mcp/clients` | 运行时连接一个外部 MCP Server |
| DELETE | `/api/mcp/clients/{name}` | 断开并卸载其工具 |

## 4. 实施情况

| 阶段 | 内容 | 产出 | 状态 |
|------|------|------|:----:|
| S1 | 配置层：`mcp` 段解析与环境变量覆盖 | `app/config.py` | 完成 |
| S2 | Server：拆分 `tools/session.py` + 新增 `tools/agents.py`，组合两层工具，支持 stdio 与 HTTP | `app/mcp/server.py`, `app/mcp/tools/` | 完成 |
| S3 | Client：`MCPToolHub` 支持 http/sse/stdio、超时、配置化批量连接 | `app/mcp/client.py` | 完成 |
| S4 | 桥接：`McpRemoteAgent` + 7 个 capability 子类注册 | `app/mcp/remote_agent.py` | 完成 |
| S5 | 接入：lifespan 托管、挂载 `/mcp`、管理端点 | `app/main.py` | 完成 |
| S6 | 文档：本文 + `docs/README.md` + `development.md` + `deployment.md` 等 | `docs/` | 完成 |
| S7 | 测试：53 项 MCP 专项用例 | `tests/test_mcp_*.py` | 完成 |

## 5. 使用指南

### 5.1 作为 stdio MCP Server（Claude Desktop / Cursor）

```json
{
  "mcpServers": {
    "tcm-consult": {
      "command": "python",
      "args": ["-m", "app.mcp.server"],
      "cwd": "/path/to/tcm_work/backend"
    }
  }
}
```

### 5.2 作为 HTTP MCP Server

启动后端后，MCP 端点位于 `http://<host>:<port>/mcp`（Streamable HTTP）。
注意该端点依赖应用 lifespan，务必以正常方式启动服务（而非直接调用 ASGI app）。

查看当前暴露的工具：

```bash
curl http://localhost:8000/api/mcp/tools
curl http://localhost:8000/api/mcp/status
```

### 5.3 接入外部 MCP Server

写进 `routing.yaml` 的 `mcp.clients` 随应用启动自动连接，或运行时接入：

```bash
curl -X POST http://localhost:8000/api/mcp/clients \
  -H 'Content-Type: application/json' \
  -d '{"name":"weather","transport":"http","url":"http://localhost:9001/mcp"}'

curl -X DELETE http://localhost:8000/api/mcp/clients/weather
```

接入后其工具以 `mcp__weather__<tool>` 出现在 SKILL 注册表中，
LLM Agent 推理时可直接 function-calling 调用。

## 6. 工程要点（踩坑记录）

1. **anyio cancel scope 跨任务问题**
   MCP 客户端传输基于 anyio task group，其 cancel scope 必须在进入它的
   *同一个任务* 中退出。若在 A 任务连接、B 任务关闭，会抛
   `Attempted to exit cancel scope in a different task`。
   因此 `MCPToolHub` 为每个连接启动**专属守护任务**，建立、持有与清理都在该任务内完成，
   通过 `asyncio.Event` 传递关闭信号。

2. **`StreamableHTTPSessionManager.run()` 只能调用一次**
   模块级复用单个 manager 会导致 uvicorn `--reload`、测试中反复创建
   TestClient 时报 `Task group is not initialized`。
   解决：`StreamableHttpEndpoint` 把"挂载对象"与"会话管理器"解耦——
   挂载对象全程稳定，manager 每次进入 lifespan 时新建。

3. **异常兜底下沉**
   统一在 `server.handle_call` 内把异常序列化为 `{"error": ...}`，
   而非只在 `@server.call_tool()` 装饰器里处理，保证 stdio 与 HTTP
   两种传输、以及直接函数调用的行为完全一致。

4. **降级不中断问诊**
   远程 Sub-Agent 的所有失败（未配 server / 未连接 / 超时 / 响应不合法）
   都由 `SubAgent.run()` 统一转为 `status=error` 信封，编排器沿用既有降级路径。

## 7. 验收结果

- [x] `python -m app.mcp.server` 可作为 stdio MCP Server 启动，`tools/list` 返回 19 个工具。
- [x] `/mcp` 可被 Streamable HTTP 客户端访问，`initialize` 握手返回 `tcm-consult`。
- [x] 7 个 capability 均有对应 `agent_*` 工具且能独立返回结构化结果。
- [x] 任一 capability 改为 `impl: mcp` 后编排器无需改动；远端不可用时自动降级。
- [x] 新增 53 项 MCP 测试全部通过；全量 225 项单元测试 + 15 项 E2E 测试无回归。
