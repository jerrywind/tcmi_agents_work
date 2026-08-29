# 项目计划与路线图（Plan & Roadmap）

> 基于 2026-08-29 的全项目审视重写。结论：**后端骨架已成形，但「前端 ↔ 后端」这条链路
> 目前是断的，且技能体系完全没有接入推理链路**。下一阶段的重心是「打通 + 提质」。
> 任务清单见 [`tasks.md`](./tasks.md)。

---

## 0. 现状概览

> 2026-08-29 更新：已完成三轮——「打通端到端 + 修复已知 bug」、
> 「技能与工具生效 + 健壮性与可观测」，以及本轮「诊疗提质」主体
> （T4.1 结构化辨证 / T4.2 兼证 / T4.4 LLM 评测集 / T4.5 MCP Server）。
> 阻断可用的 P0 问题已全部解决；剩余为语料建设与生产化类工作。

| 模块 | 状态 | 说明 |
|---|---|---|
| `server/harness/` 编排 | ✅ 可用 | 7 个 Sub-Agent + `routing.yaml` 驱动的串行编排；单步失败已可降级 |
| `server/harness/resources/*.yaml` | ✅ 可用 | 流程与数据分离，中医人员可维护 |
| `server/harness/tests/cases.rs` | ✅ 可用 | 93 条真实病例确定性回归，不依赖 LLM |
| `server/rrserver/` | ✅ 可用 | 云端中继 + 家庭端 client + 模型部署包装；资源泄漏已修 |
| `llm_server/` | ✅ 可用 | LM Studio 网关 + Agent 中间层；`rag/` 为可选子组件 |
| `deploy/` | ✅ 可用 | nginx 统一入口 + compose 编排 |
| **前端 ↔ harness 链路** | ✅ **已打通** | 旧 `api.ts` 已删除，5 个页面改用 `harness.ts` + `session.ts` 维护多轮 |
| **技能 → 推理链路** | ✅ **已接线** | 7 个 Agent 全部改用 `chat_with_tools`（此前技能在推理中完全不生效） |
| **后端构建** | ✅ **Docker 化** | 两个 Dockerfile 改为多阶段、镜像内编译，不再依赖 WSL2 预编译 |
| **MCP Client** | ✅ 已接线 | `config.yaml` 的 `mcp_clients` 启动时挂载为 `mcp__*` 工具（T2.4） |
| **MCP Server** | ✅ 已具备 | `POST /mcp` 对外暴露 7 个 `agent_*` 工具 + `run_agent` + `list_agent_capabilities`（T4.5） |
| **可观测性** | ✅ 已具备 | `/chat` 与 `POST /agents` 逐步返回 `trace`（耗时/token/模型/工具/错误，T3.1） |
| **结构化辨证** | ✅ 已具备 | 辨证步产出主证/兼证 + 置信度 + 支持/矛盾证据，随 `structured` 返回（T4.1 / T4.2） |
| **质量评测** | ⚠️ nightly | `tests/llm_eval.rs` 自动评分（需真实 LLM，不进 PR 门禁，T4.4） |

**一句话**：端到端可用、工具真正生效、调用可观测、辨证结论结构化、能力可对外暴露，
测试全绿（后端 125 / 前端 29）、构建完全 Docker 化；下一步重心是「语料建设 + 生产化」。
（后端 125 = harness 25 + rrserver 100；前端 29 含 6 条契约）

---

## 1. 问题清单

### ✅ 本轮已修复（2026-08-29）

1. **前端契约未对齐** —— 删除 `services/api.ts`（其所有端点在 harness 中不存在），
   5 个页面改接 `harness.ts`，新增 `services/session.ts` 维护多轮 `messages`。
2. **前端 18 个用例失败** —— 根因是 `vitest.setup.ts` 的 mock 返回普通 async 函数
   而非 `vi.fn()`（`mockImplementation is not a function`）；改为 `vi.hoisted()` +
   `vi.fn()`。端口从废弃的 `:22000` 改为 `:8011`。现 21/21 全绿。
3. **无多轮问诊** —— 由前端 `session.ts` 累积 messages 后重复调用 `/chat`。
4. **工具调用链路断开** —— 7 个 Agent 全部改用 `chat_with_tools`，技能真正进入推理。
5. **RAG 契约不匹配** —— `tcm-rag` 支持 `top_k`，数组响应统一包成 `{"result": [...]}`。
6. **`/chat` 无降级** —— 改为部分成功返回（`steps` + `failures` + `partial`）。
7. **死代码 / 无效配置** —— `model.rs` 删除 5 个未使用类型；`routing.yaml` 未读取的
   `default` 已移除。
8. **rrserver 资源泄漏（新发现）** —— `ChunkStream::poll_next` 里
   `let _ = reg.cancel_pending(..)` 只创建 future 随即丢弃，清理**从未执行**，
   `pending`/`streams` 持续累积。改用同步的 `cleanup_pending_sync` /
   `cleanup_stream_sync`（`try_lock`）。
9. **清单顺序随机（新发现）** —— `HashMap` 迭代顺序不稳定，
   `GET /agents` / `GET /skills` 每次启动顺序都可能变。
   现 `Registry::capabilities()` 按规范顺序、`SkillRegistry::all()` 按名排序。
10. **CI 失效** —— 重写为 Docker 化的后端测试/lint/镜像构建 + 前端测试 + 真实后端契约测试。
11. **后端构建不符合 Docker 铁律** —— 两个 Dockerfile 从「COPY WSL2 预编译二进制」
    改为多阶段镜像内编译；`build-release.ps1` 相应简化。

### ✅ 本轮已解决（2026-08-29，第三轮）

12. **MCP Server 未做** —— 新增 `POST /mcp`（`src/mcp/server.rs`），对外暴露
   7 个 `agent_*` 工具 + `run_agent` + `list_agent_capabilities`；
   `tools/call` 直接翻译成 `run_single`，不改编排器。
13. **无自动化 LLM 回归** —— 新增 `tests/llm_eval.rs`：以 `cases.jsonl` 为基准
   跑真实辨证并自动评分（按语料去重、单条超时、产出 JSON 报告），
   nightly 工作流跑分，默认不启用以免影响 PR 门禁。
14. **辨证未结构化** —— 辨证结论升级为结构化对象（主证 / 兼证 / 置信度 /
   支持证据 / 矛盾证据 / 病机 / 传变），经 `structured` 字段返回；
   矛盾证据由新增的 `resources/contradictions.yaml`（相反表现表）计算。

### 🔲 仍待处理

| # | 问题 | 任务 |
|---|---|---|
| 15 | **无 RAG 语料**：`tcm-rag` 无中文医典籍料，检索能力空转 | T4.3 |
| 16 | **无会话/报告持久化**：harness 无状态，报告不可回查 | T5.1 |

---

## 2. 阶段性路线图

### 阶段 A：打通（1–2 周）—— 让「一次完整问诊」真的能跑通
目标：**有一条用户可用的链路**。这是所有后续工作的前提。

- A1 前端契约对齐：5 个页面切到 `services/harness.ts`，`api.ts` 下线。
- A2 前端维护多轮 `messages`，实现「追问 → 回答 → 再追问」的交互闭环。
- A3 修复 vitest 配置与 Taro mock，让前端测试变绿并纳入 CI 门禁。
- A4 连真实 LM Studio 做一次人工端到端验收，产出样例问诊记录。

### 阶段 B：让技能与工具真正生效（2–3 周）
目标：**把已经写好但没接上的能力接上**，投入小、收益大。

- B1 各 Agent 改用 `chat_with_tools`，打通「LLM → 技能 → LLM」闭环。
- B2 把单轮工具调用升级为多轮循环（可配置最大轮数），支持串行查证。
- B3 为 `treatment` 补专属技能（把 `find_formula` / `find_care` 暴露为工具）。
- B4 MCP client 接线：`config.yaml` 声明式接入外部 MCP Server。
- B5 对齐 `tcm-rag` 与 RAG 服务契约（路径 + 返回结构）。

### 阶段 C：健壮性与可观测（2–3 周）
目标：**出问题能定位，出故障不雪崩**。

- C1 每步埋点：capability / 耗时 / token / 模型 / 错误，结构化日志输出。
- C2 `/chat` 部分失败降级：返回已完成步骤 + 失败步骤的错误标记。
- C3 红旗中断契约：安全门产出结构化 `blocked` 标记，供调用方决定是否终止。
- C4 死代码清理 + `routing.yaml` 语义收敛。

### 阶段 D：诊疗质量（4–6 周）
目标：**从「能跑」到「可信」**。

- D1 结构化输出：把辨证结果从自由文本升级为 JSON（证候 + 置信度 + 支持/矛盾证据链）。
- D2 兼证呈现：当前 `infer_syndrome_slug` 已返回全部候选，但未在输出中体现。
- D3 RAG 语料建设（中医典籍 / 方剂 / 医案）与召回质量评估。
- D4 LLM 评测集：把 `cases.jsonl` 扩为「输入 → 期望证候」的自动评分集，
  定期跑分跟踪质量回归（需真实 LLM，作为 nightly 而非 PR 门禁）。
- D5 MCP Server：对外暴露 7 个能力（见 [`mcp.md`](./mcp.md) 第 2.2 节）。

### 阶段 E：生产化与上线（4–6 周）
- E1 会话/报告持久化（harness 现为无状态，需要存储层 + 可选的服务器端会话）。
- E2 对象存储（图片归档）。
- E3 前端 H5 上 CDN + 微信小程序过审。
- E4 合规审计：免责强制展示、红旗路径不可移除、日志脱敏。

### 阶段 F：家庭算力云化（按需，可与 C/D/E 并行）
- F1 rrserver 生产化：真实 TLS、强 token、随机 name、告警。
- F2 多隧道 / 多模型路由。

---

## 3. 里程碑

| 里程碑 | 交付物 | 验收 |
|---|---|---|
| **M1 端到端打通** | 页面切到 harness 契约 + 多轮交互 + 前端测试变绿 | 连 LM Studio 完成一次完整问诊；`npm run test` 全绿 |
| **M2 技能生效** | 工具调用闭环 + 多轮 + MCP 接线 + RAG 对齐 | 辨证/治疗步骤的 trace 中能看到真实工具调用 |
| **M3 可观测可降级** | 埋点 + 部分失败降级 + 红旗中断契约 | 断网/LLM 超时场景下前端仍有可读结果 |
| **M4 诊疗提质** | 结构化辨证输出 + 证据链 + 兼证 + LLM 评测集 + MCP Server | ✅ 已达成（评测集待接真实 LLM 后出基线分） |
| **M5 可上线** | 持久化 + 对象存储 + 小程序发布 + 合规 | 生产 compose 起得来，CORS/HTTPS 合规 |

依赖关系：`M1 → M2 → M3`（串行，逐级加固）；`M4` 可在 `M2` 后并行启动；
`M5` 依赖 `M3`；`F` 阶段完全独立。

---

## 4. 风险与待确认

1. **模型可得性**：推理完全依赖宿主机 LM Studio。LM Studio 未启动时
   `/chat` 直接报错（无 mock 兜底），演示与验收前必须确认模型已加载。
2. **医疗合规**：AI 健康参考，非医疗诊断。上线前需法务/合规复核免责声明、
   红旗路径与日志脱敏。
3. **LLM 评测基线尚未建立**：nightly 评测集（D4）已就绪，但需在 self-hosted runner
   上连真实 LLM 跑一次以确立基线分，`HARNESS_EVAL_MIN_SCORE` 才能在 CI 中真正设门槛。
4. **知识库权威性**：方剂/剂量必须由专业中医复核，`cases.jsonl` 只保证数据存在性，
   不保证正确性。
5. **磁盘**：`server/target/` 常达数 GB，可安全删除（见 [`cleanup.md`](./cleanup.md)）。

---

## 5. 明确不做（避免过度设计）

- **不在 harness 内实现服务端会话**：先由前端维护 `messages`；确有续诊/报告留存需求时
  再在 E1 引入独立存储层。
- **不自建模型服务**：推理统一交给 LM Studio / 兼容端点。
- **不重复造 llm_server 已有的能力**：prompt 优化、多轮 agent 循环、MCP 聚合
  都在 `llm_server`；harness 侧只保留「够用的单/多轮工具调用」。
- **不做多租户 / 权限体系**：当前为单机自用形态。

---

## 6. 如何参与

- 任务看板与验收标准：[`tasks.md`](./tasks.md)
- 新增 Sub-Agent：[`agent-protocol.md`](./agent-protocol.md)
- 新增技能：[`skills.md`](./skills.md)
- 本地联调：`cd server/harness && ../target/debug/harness --listen 127.0.0.1:8011`
  + `cd frontend && npm run dev:h5`
- 测试：`cd server && cargo test`（harness + rrserver）；
  全链路 `cd e2e_tests && .\run_full_chain_e2e.ps1`
