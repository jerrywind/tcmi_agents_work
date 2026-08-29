# 风蓝科技 TCM · 中医智能问诊 Agent

> ⚠️ 免责声明：本系统由 AI 生成，仅供健康参考，**不构成医疗诊断或处方建议**。如有不适或红旗症状，请及时线下就医。

一个「望闻问切四诊 + 辨证 + 安全 + 诊疗方案」的协议化中医智能问诊系统：
前端（Taro 多端）经后端 **harness（Rust）** 调度 7 个 Sub-Agent，模型推理统一由宿主机
**LM Studio** 提供，并以 `llm_server`（网关 + Agent 中间层）为中间层；家庭算力经
`rrserver` 反向隧道上云。

> 后端为 Rust 实现 `server/harness`，隧道服务为 `server/rrserver`，
> 二者同属 `server/` Cargo workspace，统一 `cargo build` 构建。

---

## 1. 架构总览

```
┌────────────────┐     ┌───────────────────────┐
│  前端 Taro      │────▶│  harness (Rust)       │  望闻问切/辨证/安全/方案
│  H5 / 微信小程序 │     │  7× Sub-Agent 编排     │  7 大 capability
└────────────────┘     └───────────┬───────────┘
                                    │ OpenAI 兼容 (/v1/...)
                                    ▼
                          ┌───────────────────────┐   ┌──────────────────────┐
                          │  llm_server (网关)     │──▶│  LM Studio :11223    │
                          │  prompt优化/tool/MCP/  │   │  google/gemma-4-12b  │
                          │  agent 循环            │   │  (文本+视觉共用)      │
                          └───────────┬───────────┘   └──────────────────────┘
                                      │ 经家庭算力上云（可选）
                                      ▼
                          ┌───────────────────────┐
                          │  rrserver              │  云端中继 server :8088
                          │  反向隧道              │  + 家庭端 client :9000
                          └───────────────────────┘
```

| 组件 | 路径 | 角色 | 默认端口 |
|---|---|---|---|
| 前端 | `frontend/` | Taro 多端（H5 / 微信小程序），dev `:10086`、build `:8080` | 10086 / 8080 |
| 后端 | `server/harness/` | **Rust** 编排 7 个 Sub-Agent，暴露 `/chat`、`/agents`、`/skills`、`/mcp`、`/health`（nginx 以 `/api` 前缀代理） | 8011 |
| LLM 网关 | `llm_server/` | 纯 LM Studio 网关 + Agent 中间层（**不托管模型**） | 8000 |
| 模型推理 | 宿主机 LM Studio | 默认 `http://localhost:11223/v1`，模型 `google/gemma-4-12b-qat`（文本+视觉共用） | 11223 |
| 反向隧道 | `server/rrserver/` | Rust 中继：云端 server `:8088`（deploy nginx→容器`:8080`）+ 家庭端 client `:9000` | 8088 / 9000 |
| 统一入口 | `deploy/` | 独立 nginx：前端静态托管 + 反代 `/api`、`/rr`（TLS 终止），后端服务容器不含 nginx | 80 / 443 / 8080 |

> **关键事实**（各文档一致引用，避免歧义）：
> - 🔒 **后端完全依赖 Docker**：`server/harness` 与 `server/rrserver` 的构建、运行、验证
>   一律在 Docker 内完成，**不使用宿主机本地 `cargo build` 的产物**。
>   镜像为多阶段构建，在容器内编译；宿主机构建机无需安装 Rust 工具链。
> - `llm_server` **不托管/内置任何模型**，模型由宿主机 LM Studio 提供（默认 `:11223`）。
> - 模型统一为 **`google/gemma-4-12b-qat`**（原生多模态，文本与视觉共用同一端点）。
> - **harness 是无状态服务**：一次 `POST /chat` 串行跑完 `routing.yaml` 中全部激活步骤后返回，
>   **没有服务端多轮循环**，多轮由调用方累积 `messages` 实现。
> - 单步失败不中断整体：返回已完成步骤 + `failures` + `partial`；全部失败才返回 `{"error"}`。
> - **流程与数据分离**：可改内容（证候、问诊问题、方剂、调护、安全规则、相反表现、Prompt）
>   全部在 `server/harness/resources/*.yaml`，**字段 key 为英文 slug、值为中文并附中文注释**；
>   改完 `POST /reload` 或重启生效。
> - **辨证结论是结构化的**：主证 / 兼证 + 置信度 + 支持/矛盾证据随 `structured` 返回，
>   由规则层确定性产出（不经过 LLM）。
> - **无 LLM 时**：llm_server `/healthz` 返回 `degraded`、`/v1/models` 返回 503；
>   harness 的**只读端点**（`/health`、`/agents`、`/skills`）仍可用，但 `/chat` 会返回错误
>   （harness 未提供 MockProvider；确定性逻辑可用 `cargo test -p harness` 验证）。
> - **当前未接线**：MCP（仅 client 库、无调用方）、LLM 工具调用（7 个 Agent 均不调技能）、
>   调用级可观测性（无 trace / token 埋点）。详见 [`docs/tasks.md`](./docs/tasks.md)。

---

## 2. 快速开始

### 2.1 仅后端（harness，Rust）
```bash
cd server
cargo build --release                 # 构建 harness + rrserver（Windows 开发用）
# 运行 harness（cwd 需为 server/harness，resources/ 为相对路径）
cd harness && ../target/release/harness.exe --listen 0.0.0.0:8011
# 只读端点验证：http://localhost:8011/health 、/agents 、/skills
# 可改的数据均在 resources/*.yaml，改完重启或调用 POST /reload
```

### 2.1.1 Docker 部署（先编译 Linux 二进制）
```powershell
# Docker 构建容器内无法 cargo build（网络会损坏 crates.io 下载），必须先预编译：
powershell -NoProfile -File scripts\build-release.ps1     # WSL2 编译，约 90s，产物拷回 server/target/release/
cd frontend && npm run build:h5                            # 静态产物（nginx 挂载）
docker compose -f deploy/docker-compose.yml up -d --build  # nginx + harness + rrserver
```
> 构建上下文必须是 workspace 根 `server/`（产物在 `server/target/release/`，子 crate 无独立
> `target/`），上下文裁剪由 `server/.dockerignore` 负责（否则会传输数 GB 的 target）。
> 详见 [`docs/deployment.md`](./docs/deployment.md) 第 3 节。

### 2.2 接入真实 LLM（LM Studio）
```bash
# 1) LM Studio 加载 google/gemma-4-12b-qat，开启 Local Server（默认 :11223）
# 2) 启动 llm_server 网关（可选；也可直连 LM Studio）
cd llm_server && pip install -r requirements.txt && python -m app.main
# 3) harness 指向 LLM：环境变量前缀为 HARNESS_（走网关 8000，或直连 LM Studio 11223）
$env:HARNESS_LLM_BASE_URL="http://localhost:8000/v1"  # 走 llm_server 网关
$env:HARNESS_LLM_BASE_URL="http://localhost:11223/v1" # 或直连 LM Studio（默认）
$env:HARNESS_LLM_API_KEY="<LM Studio 开启了 API Key 校验时必填>"
cd server/harness && ../target/release/harness --listen 0.0.0.0:8011
```

### 2.3 前端
```bash
cd frontend
npm install
npm run dev:h5        # H5 开发服务器 :10086，apiBase 指向后端（见 config/）
```

### 2.4 家庭算力上云（rrserver，可选）
```bash
cd server/rrserver
cargo build --release                       # 需先构建二进制
# 云端中继（docker 或二进制）
.\start_rrserver.ps1                        # 启动 server(:8088) + client(:9000)
# 家庭端把本地 LLM 经隧道暴露到云端 server 的 /t/home/* 路径
```

> harness 也可直接经 rrserver 隧道暴露（无需额外家庭端进程）：
> ```bash
> cd server/harness
> ../target/release/harness --listen 0.0.0.0:8011 \
>   --tunnel-server ws://<云端 rrserver 地址> --tunnel-name tcm --tunnel-token <token>
> # 之后公网访问 https://<域名>/rr/t/tcm/* 即到达 harness
> # 也可用环境变量：HARNESS_TUNNEL_SERVER / HARNESS_TUNNEL_NAME / HARNESS_TUNNEL_TOKEN
> ```
详见 [`docs/deployment.md`](./docs/deployment.md) 的 rrserver 章节与 [`docs/plan.md`](./docs/plan.md)。

---

## 3. 文档导航

详细文档集中在 [`docs/`](./docs/)，按职责拆分，避免重复：

| 文档 | 面向 | 内容 |
|---|---|---|
| [`docs/README.md`](./docs/README.md) | 所有人 | **文档索引与职责矩阵**（先读这个） |
| [`docs/usage.md`](./docs/usage.md) | 使用者/接入方 | 前端问诊流程 + REST API 接入 + 切换真实 LLM |
| [`docs/deployment.md`](./docs/deployment.md) | 运维 | 四组件部署（本地/Docker）、端口、配置、网络 |
| [`docs/development.md`](./docs/development.md) | 开发者 | 本地开发环境、调试、常见问题 |
| [`docs/agent-protocol.md`](./docs/agent-protocol.md) | 架构/扩展 | Sub-Agent 可替换协议（capability/信封/路由） |
| [`docs/sub_agents.md`](./docs/sub_agents.md) | Agent 开发者 | 7 个 Sub-Agent 职责、System Prompt、技能绑定 |
| [`docs/skills.md`](./docs/skills.md) | 技能开发者 | SKILL 工具集契约、内置技能、热装载、自定义 |
| [`docs/mcp.md`](./docs/mcp.md) | 集成方 | 双向 MCP（对外暴露能力 / 接入外部工具） |
| [`docs/llm_server.md`](./docs/llm_server.md) | LLM 网关运维 | llm_server 网关架构、API、配置、RAG |
| [`docs/rag.md`](./docs/rag.md) | RAG 运维 | 可选 RAG 检索服务（文本/图像/图文） |
| [`docs/testing.md`](./docs/testing.md) | 测试/CI | 单元/集成/E2E 测试体系、覆盖率、CI |
| [`docs/e2e.md`](./docs/e2e.md) | 测试 | **全链路端到端测试**（前端→后端→rrserver→llm_server） |
| [`docs/cleanup.md`](./docs/cleanup.md) | 开发者 | 临时文件/日志命名与清理规范 |
| [`docs/plan.md`](./docs/plan.md) | 管理者 | 现状、路线图、里程碑、风险 |
| [`docs/tasks.md`](./docs/tasks.md) | 管理者 | 任务看板（issue 清单） |

---

## 4. 测试

- **后端（harness + rrserver，Rust）**：**一律在 Docker 内编译执行**，不使用本地 `cargo build` 产物。

  ```powershell
  # 单测 + 案例回归（cases.jsonl 93 条真实病例，不依赖 LLM）
  docker run --rm -v "${PWD}/server:/build" -w /build rust:1.98-bookworm cargo test --workspace

  # lint：fmt + clippy 严格门禁
  docker run --rm -v "${PWD}/server:/build" -w /build rust:1.98-bookworm `
    bash -c "rustup component add rustfmt clippy && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings"

  # 一键出镜像（多阶段，镜像内编译）
  pwsh scripts\build-release.ps1
  ```

  其中 `--test cases` 以 `server/harness/cases.jsonl` 做**确定性回归**：
  校验关键词证据匹配、证候推断（支持兼证）、方剂/调护检索与 YAML 资源完整性。
- **前端**：`cd frontend && npm run test`（vitest，当前 **29 个用例全绿**；
  契约测试在后端不可达时自动 skip）。详见 [`docs/testing.md`](./docs/testing.md)。
- **全链路 E2E**（rrserver / llm_server / harness）：`e2e_tests/` 下 pytest，一键编排
  `run_full_chain_e2e.ps1`（默认不含 rrserver 与前端；`-WithRrserver` / `-WithFrontend` 开启）。
  详见 [`docs/e2e.md`](./docs/e2e.md)。

---

## 5. 许可证与合规

AI 健康参考，非医疗诊断。上线前需完成合规复核（免责强制展示、红旗中断路径不可移除、日志脱敏）。

---

## 6. 目录说明

| 路径 | 说明 |
|---|---|
| `server/harness/` | 诊断编排后端（Rust）：`src/` 程序逻辑、`resources/*.yaml` 可改数据、`cases.jsonl` 回归基准 |
| `server/rrserver/` | 反向隧道（Rust）：云端 server + 家庭端 client + 模型部署包装 |
| `frontend/` | Taro 多端（H5 / 微信小程序） |
| `llm_server/` | LM Studio 网关 + Agent 中间层（Python），`rag/` 为可选子组件 |
| `deploy/` | 统一 nginx 入口（静态托管 + 反代 /api、/rr + TLS 终止）与 compose 编排 |
| `e2e_tests/` | 全链路 E2E（harness → rrserver → llm_server） |
| `scripts/` | `build-release.ps1`（WSL2 预编译）、`cleanup.ps1`（清理） |
| `docs/` | 文档，见 [`docs/README.md`](./docs/README.md) |

> **流程与数据分离**：证候、问诊问题、方剂、调护、安全规则、System Prompt 全部在
> `server/harness/resources/*.yaml`（英文 slug key + 中文值 + 中文注释），
> 中医专业人员无需改代码即可维护；改完 `POST /reload` 或重启生效。
