# 任务看板（Task Board）

> 对应 [`plan.md`](./plan.md) 的阶段路线图。状态约定：🔲 待办 / 🔧 进行中 / ✅ 已完成 / 🚫 阻塞。
> 最后更新：2026-08-29（本轮完成「诊疗提质」主体：
> T4.1 结构化辨证 / T4.2 兼证 / T4.4 LLM 评测集 / T4.5 MCP Server，
> 并把结构化结论接到前端报告页）

## ⚠️ 铁律

**后端（`server/harness` 与 `server/rrserver`）完全依赖 Docker 构建、运行与验证，
禁止用宿主机本地 `cargo build` 的产物作为交付或验证依据。**
任何后端改动都必须在 Docker 内编译并测试通过。镜像采用多阶段构建，在容器内完成编译。

## 阶段 A：打通端到端（M1）

| # | 任务 | 验收 | 状态 |
|---|---|---|---|
| T1.0 | 修复前端测试基线：`vitest.setup.ts` 的 mock 必须是 `vi.fn()`（此前是普通 async 函数，导致 `vi.mocked(...).mockImplementation is not a function`）；`vitest.config.ts` 端口从废弃的 `:22000` 改为 `:8011` | `npm run test` 全绿 | ✅ 已完成（21/21） |
| T1.1 | 下线旧契约 `services/api.ts`（所有端点在 harness 中不存在），5 个页面改接 `services/harness.ts` | 无页面引用已删除的 api.ts | ✅ 已完成 |
| T1.2 | 前端维护多轮 `messages`（harness 无服务端循环），新增 `services/session.ts` | 页面可完成「追问 → 回答 → 再追问」 | ✅ 已完成 |
| T1.3 | 前端解析 `steps[]` 分步渲染 + `summary` | 7 步可切换查看 | ✅ 已完成 |
| T1.4 | 启用前端 e2e：编排脚本改为 Docker 起 harness（`tcm-harness:e2e`），前端契约测试默认开启，`-SkipFrontend` 关闭；`-WithFrontend` 保留为兼容参数 | `pytest=0 frontend=0`（4 + 4 用例全绿） | ✅ 已完成 |
| T1.5 | 连真实 LM Studio 做一次端到端人工验收，产出样例问诊记录 | 记录归档 | 🔲 待办 |
| T1.6 | 移除 CI 中 frontend job 的 `continue-on-error` | CI 门禁生效 | ✅ 已完成 |
| T1.7 | 补齐 CI：`cargo fmt --check` + `clippy -D warnings` | lint job 通过 | ✅ 已完成（Docker 内执行） |
| T1.8 | 清理死代码：`model.rs` 的 5 个未使用类型 | 编译通过 | ✅ 已完成 |
| T1.9 | 修复 `server/rrserver/start_rrserver.ps1`：路径仍指向迁移前的 `tcm_work/rrserver` 与错误的 `rrserver\target` | 脚本可一键启动 | ✅ 已完成 |

## 阶段 B：让技能与工具真正生效（M2）

| # | 任务 | 验收 | 状态 |
|---|---|---|---|
| T2.0 | 7 个 Agent 改用 `chat_with_tools`（此前全调 `chat_completion`，9 个技能在推理中完全不生效） | 各步 trace 可见工具调用 | ✅ 已完成 |
| T2.1 | 对齐 `tcm-rag` 与 RAG 服务契约：支持 `top_k`，数组响应统一包成 `{"result": [...]}` | 配置端点后返回结构化结果 | ✅ 已完成 |
| T2.2 | 工具调用升级为多轮循环（此前仅「1 次带工具 + 1 次汇总」） | `max_tool_rounds` 可配（默认 3） | ✅ 已完成（`LlmCaller::chat_with_tools` 循环，达上限后转汇总调用） |
| T2.3 | 为 `treatment` 补专属技能（`find_formula` / `find_care` 暴露为工具） | 治疗步可调用方剂工具 | ✅ 已完成（新增 `tcm-formula` / `tcm-care`，owner=treatment） |
| T2.4 | MCP client 接线：`config.yaml` 增加 `mcp_clients` | `GET /skills` 出现 `mcp__*` | ✅ 已完成（`mount_mcp_clients` 启动时 `tools/list` 挂载；实测 `mcp__kb__search_kb` 可见可调用） |
| T2.5 | `POST /skills` 按 owner 过滤（此前不过滤） | 专属技能受 owner 约束 | ✅ 已完成（`POST /skills` 与 `GET /skills?owner=` 均支持，越界返回「未知技能」） |

## 阶段 C：健壮性与可观测（M3）

| # | 任务 | 验收 | 状态 |
|---|---|---|---|
| T3.1 | 每步埋点：capability / 耗时 / token / 模型 / 错误 | 一次 `/chat` 产出 7 条结构化记录 | ✅ 已完成（新增 `src/trace.rs`；`/chat` 与 `POST /agents` 响应带 `trace[]`，并写 tracing 日志） |
| T3.2 | `/chat` 部分失败降级：返回已完成步骤 + `failures` + `partial` | LLM 中途超时仍有可用结果 | ✅ 已完成 |
| T3.3 | 红旗中断契约：安全门产出结构化 `blocked` 标记 | 调用方可据此终止 | ✅ 已完成（high/critical 命中后跳过后续步骤，响应带 `blocked`/`block_reason`/`skipped[]`；`medium` 仅告警） |
| T3.4 | 收敛 `routing.yaml`：未被读取的 `default` 字段已删除 | 配置与行为一致 | ✅ 已完成 |
| T3.5 | LLM 调用超时重试 | 单次抖动不影响整次问诊 | ✅ 已完成（`llm_max_retries` 默认 2 + 指数退避；仅超时/连接失败/5xx/429 重试，4xx 不重试） |
| T3.6 | **rrserver 资源泄漏**：`ChunkStream::poll_next` 里 `let _ = reg.cancel_pending(..)` 只创建 future 随即丢弃，清理从不执行，`pending`/`streams` 持续累积 | 长跑无泄漏 | ✅ 已完成（改用 `cleanup_pending_sync` / `cleanup_stream_sync`） |
| T3.7 | **能力/技能清单顺序不稳定**：`HashMap` 迭代顺序每次进程启动都可能不同，`GET /agents`、`GET /skills` 返回顺序随机 | 顺序稳定 | ✅ 已完成（`Registry::capabilities()` 按规范顺序；`SkillRegistry::all()` 按名排序） |

## 阶段 D：诊疗质量（M4）

| # | 任务 | 验收 | 状态 |
|---|---|---|---|
| T4.1 | 辨证结构化输出（证候 + 置信度 + 支持/矛盾证据） | `differentiation` 步可解析为对象 | ✅ 已完成（`src/agents/differentiation.rs::assess` 纯函数打分；新增 `resources/contradictions.yaml` 相反表现表产出矛盾证据；经 `SubAgent::structured` 随 `/chat`、`POST /agents` 返回 `structured.differentiation`） |
| T4.2 | 兼证呈现 | 报告可展示多证候并存 | ✅ 已完成（主证之外，置信度达标且证据量 ≥ 主证 60% 的候选列为 `concurrent`；前端报告页卡片化展示主证/兼证与证据链） |
| T4.3 | RAG 语料建设与召回质量评估 | 有评估样例集 | 🔲 待办（需先有语料，属内容建设而非代码） |
| T4.4 | LLM 评测集（用 `cases.jsonl` 自动评分），nightly 跑分 | nightly 产出质量分 | ✅ 已完成（`tests/llm_eval.rs`，`HARNESS_EVAL=1` 启用，默认跳过；`.github/workflows/llm-eval.yml` nightly + 手动触发，跑在 self-hosted runner 上） |
| T4.5 | MCP Server：对外暴露 7 个 `agent_*` 工具 | MCP 客户端可调用 | ✅ 已完成（`POST /mcp`，`src/mcp/server.rs`；7 个 `agent_*` + `run_agent` + `list_agent_capabilities`） |

## 阶段 E：生产化（M5）

| # | 任务 | 验收 | 状态 |
|---|---|---|---|
| T5.1 | 会话 / 报告持久化层 | 报告可回查 | 🔲 待办 |
| T5.2 | 对象存储（图片归档） | 上传走 OSS/S3 | 🔲 待办 |
| T5.3 | 前端 H5 上 CDN + 微信小程序过审 | 线上可访问 | 🔲 待办 |
| T5.4 | 合规审计：免责强制展示、红旗路径不可移除、日志脱敏 | 安全评审通过 | 🔲 待办 |

## 阶段 F：家庭算力云化（可并行）

| # | 任务 | 验收 | 状态 |
|---|---|---|---|
| T6.1 | rrserver 生产化：真实 TLS、强 token / 随机 name、告警 | 公网隧道稳定 | 🔲 待办 |
| T6.2 | 多隧道 / 多模型路由 | 可按 capability 选路 | 🔲 待办 |
| T6.3 | harness 隧道加固：重连 / 鉴权 / 观测 | 断线自动重连 | 🔲 待办 |

## 基础设施

| # | 任务 | 验收 | 状态 |
|---|---|---|---|
| I.1 | **后端 Dockerfile 改为镜像内编译**：此前 COPY 的是 WSL2 预编译二进制，违背「后端完全依赖 Docker」 | `docker build` 独立完成编译 | ✅ 已完成（harness + rrserver 均改为多阶段） |
| I.2 | `scripts/build-release.ps1` 去掉 WSL2 预编译，改为直接 `docker build` + Docker 内测试 | 脚本可一键出镜像 | ✅ 已完成 |
| I.3 | `server/.dockerignore` 改为放行源码、只排除 `target/` | 上下文精简 | ✅ 已完成 |
| I.4 | Rust 工具链升级到 1.98.0，依赖 `cargo update` | 构建通过 | ✅ 已完成（依赖已为最新，0 个待更新） |
| I.5 | 清理本地 `server/target/`（3.2 GB，后端已 Docker 化后不再需要） | 释放磁盘 | ✅ 已完成 |
| I.6 | 清理遗留容器与镜像：旧 Python backend（`backend-backend-1`，宿主 22000）、旧 harness 冒烟容器（`tcm-harness-smoke`） | 无残留容器 | ✅ 已完成 |
| I.7 | 清理后恢复服务：镜像被一并 prune，需 `docker build` 重建并重新拉起 8011 | `/health`、`/agents`、`/skills` 正常 | ✅ 已完成（验证了 Docker 化构建可从零复现） |

## 已完成（上一轮：文档与清理）

| # | 任务 |
|---|---|
| ✅ | 重写 `skills.md` / `mcp.md` / `agent-protocol.md` / `sub_agents.md`（此前通篇描述已删除的 Python backend） |
| ✅ | 清除 `_useless/` 引用（该目录已不存在） |
| ✅ | 重写 CI 工作流与 Postman 集合 |
| ✅ | 删除失效文件（日志残片、`.env.example`、`.mcp.json`、`test_backend_llm_integration_e2e.py`、`rrserver/Cargo.lock`、`config.toml`） |
| ✅ | 清理 `.gitignore` |

## 依赖关系

```
T1.0 → T1.1 → {T1.2, T1.3} → T1.4 → T1.5
T2.2 依赖 T2.0
T4.2 依赖 T4.1
T3.x 与 T4.x 可并行
T6.x 完全独立
```

## 如何更新本表

- 完成某项：状态改为 ✅ 并注明完成日期与关键做法（便于回溯）。
- 新增任务：按阶段插入，编号 `T<阶段>.<序号>`。
- 阻塞项：标 🚫 并在「验收」列写明阻塞原因。
