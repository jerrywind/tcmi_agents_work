# 风蓝科技 TCM · 中医智能问诊 Agent

> ⚠️ 免责声明：本系统由 AI 生成，仅供健康参考，**不构成医疗诊断或处方建议**。如有不适或红旗症状，请及时线下就医。

「望闻问切四诊 + 辨证 + 安全门 + 诊疗方案」的协议化中医智能问诊系统：
前端（Taro 多端）经后端 **harness（Rust）** 调度 7 个 Sub-Agent，模型推理由宿主机
**LM Studio** 提供（`llm_server` 为可选网关），家庭算力经 `rrserver` 反向隧道上云。

---

## 1. 架构总览

```
┌────────────────┐     ┌───────────────────────┐
│  前端 Taro      │────▶│  harness (Rust)       │  望闻问切/辨证/安全门/治疗
│  H5 / 微信小程序 │     │  7× Sub-Agent 编排     │
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
                          │  rrserver 反向隧道      │  云端 server :8088
                          │                        │  + 家庭端 client :9000
                          └───────────────────────┘
```

| 组件 | 路径 | 角色 | 默认端口 |
|---|---|---|---|
| 前端 | `frontend/` | Taro 多端（H5 / 微信小程序），dev `:10086` | 10086 |
| 后端 | `server/harness/` | Rust 编排 7 个 Sub-Agent（nginx 以 `/api` 前缀代理） | 8011 |
| LLM 网关 | `llm_server/` | 纯 LM Studio 网关 + Agent 中间层（**不托管模型**，可选） | 8000 |
| 模型推理 | 宿主机 LM Studio | `http://localhost:11223/v1`，模型 `google/gemma-4-12b-qat` | 11223 |
| 反向隧道 | `server/rrserver/` | Rust 中继：云端 server `:8088` + 家庭端 client `:9000` | 8088 / 9000 |
| 统一入口 | `deploy/` | 独立 nginx：静态托管 + 反代 `/api`、`/rr`（TLS 终止） | 80 / 443 / 8080 |

harness 端点：`/health`、`/agents`(GET/POST)、`/chat`、`/skills`(GET/POST)、
`/mcp`、`/reload`、`/reports`、`/reports/:id`（完整契约见 [`docs/usage.md`](./docs/usage.md)）。

### 关键事实（各文档一致引用）

- 🔒 **后端完全依赖 Docker**：harness 与 rrserver 的构建、运行、验证一律在 Docker 内完成，
  **不使用宿主机 `cargo build` 产物**。镜像为多阶段构建（容器内编译），构建机无需 Rust 工具链。
- **harness 无状态**：一次 `POST /chat` 串行跑完 `routing.yaml` 全部激活步骤即返回，
  **没有服务端多轮循环**——多轮由调用方累积 `messages`。报告持久化默认关闭。
- **单步失败不中断**：返回已完成步骤 + `failures` + `partial`；全部失败才返回 `{"error"}`。
- **辨证结论是结构化的**：主证 / 兼证 + 置信度 + 支持/矛盾证据随 `structured` 返回，
  由规则层确定性产出（不经 LLM，可回归测试）。
- **合规底线**：免责声明随每份结果下发；安全门**不可从 `routing.yaml` 移除**
  （缺失时强制插入并告警）；落盘内容脱敏。
- **无 LLM 时**：`/chat` 会失败（harness 无 MockProvider），只读端点仍可用；
  llm_server `/healthz` 返回 `degraded`、`/v1/models` 返回 503。
- **流程与数据分离**：证候、问诊问题、方剂、调护、安全规则、相反表现、Prompt 全在
  `server/harness/resources/*.yaml`（英文 slug key + 中文值 + 中文注释），
  改完 `POST /reload` 或重启生效。

---

## 2. 快速开始

**前置**：Docker（后端必需）、Node 18+（前端）、Python 3.11+（可选网关/RAG）、
LM Studio（真实推理）。

```powershell
# 1) 后端：Docker 内编译并出镜像（多阶段，无需本地 Rust）
cd server
docker build -f harness/Dockerfile -t tcm-harness:local .
docker run -d --name tcm-harness-8011 -p 8011:8011 `
  -e HARNESS_LLM_BASE_URL=http://host.docker.internal:11223/v1 `
  -e HARNESS_LLM_API_KEY=<LM Studio 令牌> `
  tcm-harness:local

# 2) 验证：http://127.0.0.1:8011/health 返回 ok
#    容器内访问宿主机 LM Studio 用 host.docker.internal（不是 localhost）

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

---

## 4. 测试

```powershell
# 后端 129 用例（harness 29 + rrserver 100）+ fmt + clippy 严格门禁，全部在 Docker 内
docker run --rm -v "${PWD}/server:/build" -w /build rust:1.98-bookworm `
  bash -c "rustup component add rustfmt clippy && cargo fmt --all -- --check && `
           cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace"

# 前端 32 用例
cd frontend && npm run test

# RAG 语料 12 用例 + 检索服务 6 用例
cd llm_server/rag && python -m unittest test_corpus test_rag
```

- `--test cases` 以 `cases.jsonl`（93 条真实病例）做**确定性回归**，不依赖 LLM。
- 全链路 E2E：`e2e_tests/run_full_chain_e2e.ps1`（用 stub，无需真实 LLM）。
- **人工验收**（需真实 LLM）：`e2e_tests/run_manual_e2e.ps1 -Case damp-heat`，
  产出归档在 [`docs/samples/`](./docs/samples/README.md)。

详见 [`docs/testing.md`](./docs/testing.md)。

---

## 5. 目录说明

| 路径 | 说明 |
|---|---|
| `server/harness/` | 诊断编排后端（Rust）：`src/` 逻辑、`resources/*.yaml` 可改数据、`cases.jsonl` 回归基准 |
| `server/rrserver/` | 反向隧道（Rust）：云端 server + 家庭端 client + 模型部署包装 |
| `frontend/` | Taro 多端（H5 / 微信小程序） |
| `llm_server/` | LM Studio 网关（Python，可选），`rag/` 为其检索子组件 |
| `deploy/` | 统一 nginx 入口（静态托管 + 反代 /api、/rr + TLS 终止）与 compose 编排 |
| `e2e_tests/` | 全链路 E2E 与人工验收脚本 |
| `scripts/` | `build-release.ps1`（Docker 出镜像）、`cleanup.ps1`（清理） |
| `docs/samples/` | 连真实 LLM 跑出的端到端样例，用于回归对照 |
| `rag_data/` | 中医典籍语料（700 部 / 6618 万字），**不入库**，见 `docs/rag.md` |

---

## 6. 许可证与合规

AI 健康参考，非医疗诊断。上线前需完成合规复核：免责声明强制展示、
红旗中断路径不可移除、日志脱敏、报告保留期。
检查清单见 `docs/deployment.md` 第 7 节。
