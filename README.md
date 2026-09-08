# 风蓝科技 TCM · 中医智能问诊 Agent

> 🌐 **简体中文** | [English](./README.en.md)

> ⚠️ 免责声明：本系统由 AI 生成，仅供健康参考，**不构成医疗诊断或处方建议**。如有不适或红旗症状，请及时线下就医。

「望闻问切四诊 + 辨证 + 安全门 + 诊疗方案」的协议化中医智能问诊系统：
前端（Taro 多端）经后端 **harness（Rust）** 调度 13 个 Sub-Agent，模型推理由宿主机
**LM Studio** 提供（`llm_server` 为可选网关），家庭算力经 `rrserver` 反向隧道上云。

---

## 1. 架构总览

```
┌────────────────┐     ┌───────────────────────┐
│  前端 Taro      │────▶│  harness (Rust)       │  望闻问切/辨证/安全门/治疗
│  H5 / 微信小程序 │     │  13× Sub-Agent 编排    │
└────────────────┘     └───────────┬───────────┘
                                    │ OpenAI 兼容 (/v1/...)
                                    ▼
                          ┌───────────────────────┐   ┌──────────────────────┐
                          │  llm_server (可选网关)  │──▶│  LM Studio :11223    │
                          │  prompt优化/tool/MCP/  │   │  google/gemma-4-12b  │
                          │  agent 循环            │   │  (文本+视觉共用)      │
                          └───────────┬───────────┘   └──────────────────────┘
                                      │ 经家庭算力上云（可选）
                                      ▼
                          ┌───────────────────────┐
                          │  rrserver 反向隧道      │  云端 server :43302
                          │                        │  + 家庭端 client :9000
                          └───────────────────────┘
```

| 组件 | 路径 | 角色 | 默认端口 |
|---|---|---|---|
| 前端 | `frontend/` | Taro 多端（H5 / 微信小程序），dev `:10086` | 10086 |
| 后端 | `server/`（统一镜像 `tcmi_server`） | harness Rust 编排 13 个 Sub-Agent + rrserver 云端中继（同容器） | 43301 / 43302 |
| LLM 网关 | `llm_server/` | 纯 LM Studio 网关 + Agent 中间层（**不托管模型**，可选） | 8000 |
| 模型推理 | 宿主机 LM Studio | `http://localhost:11223/v1`，模型 `google/gemma-4-12b-qat` | 11223 |
| 反向隧道 | `server/rrserver/` | Rust 中继：云端 server `:43302` + 家庭端 client `:9000`（与 harness **同一镜像 / 容器**） | 43302 / 9000 |
| 统一入口 | `deploy/` | 独立 nginx：静态托管 + 反代 `/api`、`/rr`（TLS 终止） | 80 / 443 / 8080 |

harness 端点（11 条路由）：`/health`、`/agents`(GET/POST)、`/chat`、`/chat/stream`(SSE)、
`/skills`(GET/POST)、`/mcp`、`/reload`、`/reports`、`/reports/:id`
（完整契约见 [`docs/usage.md`](./docs/usage.md)）。

### 关键事实（各文档一致引用）

- 🔒 **后端完全依赖 Docker**：harness 与 rrserver 的构建、运行、验证一律在 Docker 内完成，
  **不使用宿主机 `cargo build` 产物**。两者由**同一个多阶段镜像 `tcmi_server`**
  （`server/Dockerfile`，容器内编译）一次产出，并默认在**同一个容器**里运行
  （harness `43301` 对外通信 / rrserver `43302` 中继），构建机无需 Rust 工具链。
- **harness 无状态**：一次 `POST /chat` 串行跑完 `routing.yaml` 激活档位的全部步骤即返回，
  **没有服务端多轮循环**——多轮由调用方累积 `messages` 并递增 `payload.round`。报告持久化默认关闭。
- **反馈式辨证**：信息不足时流程**停在辨证之后**并返回 `status: "awaiting_input"` +
  `loop.pending_questions`，此时**不给治疗建议**；达到轮次上限（默认 3）强制放行并标 `forced`。
- **单步失败不中断**：返回已完成步骤 + `failures` + `partial`；全部失败才返回 `{"error"}`。
- **辨证结论是结构化的**：主证 / 兼证 + 置信度 + 支持/矛盾证据随 `structured` 返回，
  由规则层确定性产出（不经 LLM，可回归测试）。库外病例会**诚实降级**（`primary=null` + `near`），
  不硬塞一个证候。
- **流式输出**：`POST /chat/stream`（SSE）逐步推送 `step_start` / `step_delta` /
  `step_retry` / `step_done` / `confidence` / `summary` / `done`，前端可边算边看；
  上游不支持 SSE 时自动退回整包解析（`HARNESS_LLM_STREAM=false` 可关）。
- **合规底线**：免责声明随每份结果下发；安全门**不可从 `routing.yaml` 移除**
  （缺失时强制插入并告警），且固定排在采集期之后、辨证期之前；落盘内容脱敏。
- **无 LLM 时**：`/chat` 会失败（harness 无 MockProvider），只读端点仍可用；
  llm_server `/healthz` 返回 `degraded`、`/v1/models` 返回 503。
- **流程与数据分离**：证候（16 个）、方剂（36 首）、问诊问题（11 条）、调护（16 组）、
  安全规则（6 条红旗）、相反表现（14 对）、传变（3 条）、Prompt（14 段）、检索域（13 个）
  全在 `server/harness/resources/*.yaml`（英文 slug key + 中文值 + 中文注释），
  改完 `POST /reload` 或重启生效。
- **11 个内置技能**（编译期注册）+ `mcp_clients` 挂载的外部 `mcp__*` 工具；
  13 个 Sub-Agent 全部走 `chat_with_tools`，推理时可真实调用工具。

---

## 2. 快速开始

**前置**：Docker（后端必需）、Node 18+（前端）、Python 3.11+（可选网关/RAG）、
LM Studio（真实推理）。

```powershell
# 1) 后端：Docker 内编译并出镜像（多阶段，无需本地 Rust）
#    统一镜像 tcmi_server 内含 harness + rrserver 两个二进制，默认同容器启动：
#    harness 43301（对外通信）/ rrserver 43302（中继）
cd server
docker build -t tcmi_server:local .
docker run -d --name tcmi_server -p 43301:43301 -p 43302:43302 `
  -e HARNESS_LLM_BASE_URL=http://host.docker.internal:11223/v1 `
  -e HARNESS_LLM_API_KEY=<LM Studio 令牌> `
  tcmi_server:local

# 2) 验证：http://127.0.0.1:43301/health 返回 {"status":"ok","rag":{...}}
#    容器内访问宿主机 LM Studio 用 host.docker.internal（不是 localhost）
#    ⚠️ 验证一律用 127.0.0.1：Windows 上 localhost 会先解析 IPv6 ::1，回落要等约 21 秒

# 3) 前端
cd frontend && npm install && npm run dev:h5     # http://localhost:10086
```

一键出镜像：`pwsh scripts\build-release.ps1`（等价于上面的 `docker build`）。
部署、端口、环境变量详见 [`docs/deployment.md`](./docs/deployment.md)。

> 家庭算力上云（可选）：见 `docs/deployment.md` 第 5 节。

---

## 3. 文档导航

文档集中在 `docs/`，按职责拆分、单一事实只在一处定义。
**完整索引与职责矩阵见 [`docs/README.md`](./docs/README.md)**，高频入口：

| 我想… | 看这里 |
|---|---|
| 跑起来 / 接 API | [`usage.md`](./docs/usage.md) |
| 部署上线 | [`deployment.md`](./docs/deployment.md) |
| 本地开发 / 踩坑 | [`development.md`](./docs/development.md) |
| 改 Agent / 技能 / 协议 | [`agent-protocol.md`](./docs/agent-protocol.md)、[`sub_agents.md`](./docs/sub_agents.md)、[`skills.md`](./docs/skills.md) |
| 接 MCP / 用 RAG | [`mcp.md`](./docs/mcp.md)、[`rag.md`](./docs/rag.md) |
| 保证质量 | [`testing.md`](./docs/testing.md)、[`e2e.md`](./docs/e2e.md)、[`samples/`](./docs/samples/README.md) |
| 看进度 / 规划 | [`plan.md`](./docs/plan.md)、[`tasks.md`](./docs/tasks.md) |

> **语言**：本 README 有[英文版](./README.en.md)；`docs/` 目前仅中文。
> 见第 7 节「文档语言与 i18n」。

---

## 4. 测试

```powershell
# 后端 233 用例（harness 86 + rrserver 147）+ fmt + clippy 严格门禁，全部在 Docker 内
docker run --rm -v "${PWD}/server:/build" -w /build rust:1.98-bookworm `
  bash -c "rustup component add rustfmt clippy && cargo fmt --all -- --check && `
           cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace"

# 前端 132 用例（11 个文件；其中 8 条契约需本机先起 harness 才真跑，否则自动 skip）
cd frontend && npm run test

# llm_server（8 条，pytest）+ RAG 子组件（65 条，unittest）
cd llm_server && python -m pytest tests -q
cd llm_server/rag && python -m unittest test_corpus test_rag test_taxonomy test_api_scope
```

- `--test cases` 以 `cases.jsonl` 做**资源完整性护栏**（期望证候在库内、有方剂或调护、
  关键词能命中），不依赖 LLM。注意它是**合成基准**：93 条只有 5 种主诉、3 种证候组合，
  其中 37 条主诉是占位符 `x`——它守护的是「数据没写漏」，不是「辨证辨得对」，
  详见 [`testing.md`](./docs/testing.md)。
- `golden_cases.jsonl`（21 条）断言**首位命中**与「库外不得出证」，补的是 `cases.jsonl`
  守不住的那一块。
- 全链路 E2E：`e2e_tests/run_full_chain_e2e.ps1`（用 stub，无需真实 LLM）。
- **人工验收**（需真实 LLM）：`e2e_tests/run_manual_e2e.ps1 -Case damp-heat`，
  产出归档在 [`docs/samples/`](./docs/samples/README.md)。

详见 [`docs/testing.md`](./docs/testing.md)。

---

## 5. 目录说明

| 路径 | 说明 |
|---|---|
| `server/harness/` | 诊断编排后端（Rust）：`src/` 逻辑、`resources/*.yaml` 可改数据、`cases.jsonl` 合成护栏、`golden_cases.jsonl` 黄金病例集 |
| `server/rrserver/` | 反向隧道（Rust）：云端 server + 家庭端 client + 模型部署包装 |
| `frontend/` | Taro 多端（H5 / 微信小程序）：6 个页面 + `services/{harness,session,stream,members}.ts` |
| `llm_server/` | LM Studio 网关（Python，可选），`rag/` 为其检索子组件 |
| `deploy/` | 统一 nginx 入口（静态托管 + 反代 /api、/rr + TLS 终止）与 compose 编排 |
| `e2e_tests/` | 全链路 E2E 与人工验收脚本 |
| `scripts/` | `build-release.ps1`（Docker 出镜像）、`cleanup.ps1`（清理） |
| `docs/samples/` | 连真实 LLM 跑出的端到端样例，用于回归对照 |
| `rag_data/` | 中医典籍语料（694 部 txt，索引 696 部 / 6618 万字），**不入库**，见 `docs/rag.md` |

---

## 6. 许可证与合规

AI 健康参考，非医疗诊断。上线前需完成合规复核：免责声明强制展示、
红旗中断路径不可移除、日志脱敏、报告保留期。
检查清单见 `docs/deployment.md` 第 7 节。

---

## 7. 文档语言与 i18n

| 文件 | 语言 | 说明 |
|---|---|---|
| [`README.md`](./README.md) | 简体中文 | 中文版入口（本文件） |
| [`README.en.md`](./README.en.md) | English | 英文版入口，与中文版**同结构、同章节号**，内容一一对应 |
| `docs/*.md` | 简体中文 | 详细文档，尚未翻译；英文版 README 第 3 节给出英文的文档速查表 |

约定：

- 两份 README 顶部都带语言切换行（`🌐 简体中文 | English`），互为链接；
- **改一处要同步另一处**：章节号与关键数字（端口、用例数、资源条目数）必须一致；
- 新增语言时复制 `README.en.md` 为 `README.<lang>.md` 并登记到本节表格与另一版的语言切换行。
