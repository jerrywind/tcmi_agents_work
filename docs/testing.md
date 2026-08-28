# 测试体系（Testing）

覆盖单元、集成、案例回归与**全链路 E2E**。测试目标与质量门禁：

- **后端（harness，Rust）测试全绿**：`cargo test -p harness`。
- 关键路径 0 回归：诊断编排流程、7 个 Sub-Agent、MCP/工具调用、skills 注册、
  PPG 解析、十八反十九畏与妊娠禁忌校验、YAML 资源完整性。
- 临时文件/日志命名与清理见 [`cleanup.md`](./cleanup.md)。

> 后端已由 Python（原 `backend/`，归档于 `_useless/backend/`）重写为 Rust
> （`server/harness`）。原 `backend/tests/**`（pytest）随实现一并归档，不再维护；
> 下文给出 Rust 侧的等价测试体系。

---

## 1. 后端 harness（Rust）

| 层级 | 位置 | 说明 |
|---|---|---|
| 单元 | `server/harness/src/**` 的 `#[cfg(test)]` | PPG 解析、配伍禁忌校验、证候/方剂检索、关键词证据匹配、工具调用 |
| 集成 | `server/harness/tests/cases.rs` | **案例回归**：以 `cases.jsonl` 真实病例校验资源与纯函数链路（不依赖 LLM） |
| 隧道 | `server/rrserver/tests/integration.rs` | rrserver 端到端：注册鉴权、隧道转发、流式、CORS、断线重连 |

```bash
cd server
cargo test                              # 整个 workspace（harness + rrserver）
cargo test -p harness                   # 仅 harness
cargo test -p harness --test cases      # 仅案例回归
cargo test -p rrserver                  # 仅 rrserver
```

### 1.1 案例回归（`--test cases`）

基准数据 `server/harness/cases.jsonl`（源自原 backend 的真实会诊记录，93 条），校验：

1. **关键词证据匹配**：案例主诉 + 证据能命中 `keywords.yaml` 的证据标签。
2. **证候推断**：`infer_syndrome_slug` 返回的候选集覆盖案例期望证候（**支持兼证**，
   返回全部得分 > 0 的证候，按得分降序）。
3. **方剂/调护检索**：每个期望证候能从 `formulas.yaml` / `care.yaml` 检索到内容。
4. **YAML 资源完整性**：案例中出现的证候必须在 `syndromes.yaml` 存在——
   缺数据时测试**直接失败并列出缺失证候**，是资源维护的回归护栏。

该测试**不依赖 LLM**（纯函数 + 资源数据），可在 CI 中稳定运行。
新增/调整 YAML 资源后跑它，即可确认未破坏既有病例。

> 需真实 LLM 的问诊链路（`/chat`）不在自动化测试内：harness 未提供 MockProvider，
> 请连 LM Studio 后手工验证。

---

## 2. 前端（vitest）

```bash
cd frontend
npm install
npx vitest run            # 单测（jsdom）
npx vitest run src/services/api.e2e.test.ts   # 前端 service 层 ↔ 后端 函数级 e2e
```
- 组件/逻辑单测用 `vitest + jsdom`，mock Taro API。
- `api.e2e.test.ts` 把 Taro 适配层替换为真实 fetch，对接已启动 backend（设 `TCM_API_BASE`），
  真实执行 `src/services/api.ts`，验证前端契约 ↔ 后端一致（详见 [`e2e.md`](./e2e.md)）。

---

## 3. 全链路 E2E（rrserver→llm_server→harness）

独立套件位于 `tcm_work/e2e_tests/`，一键编排 `run_full_chain_e2e.ps1`：

- `test_rrserver_e2e.py`：rrserver 隧道转发（需 Rust 编译产物，缺失则 skip）
- `test_llm_server_e2e.py`：llm_server 网关健康检查与透传（无上游→`degraded`/`503`）
- `test_backend_llm_integration_e2e.py`：**已归档**——针对原 backend 的 Python 契约
  （`/api/healthz`、`/api/consultations`、上传等），默认以 `-k "not backend"` 排除；
  harness 侧的等价覆盖由 `cargo test -p harness` 承担。
- 前端：`frontend/src/services/api.e2e.test.ts`（vitest，**默认跳过**，
  因 `api.ts` 仍按旧 backend 契约，用 `-WithFrontend` 强制开启）

设计要点：**无真实 LLM 也能跑通**——rrserver/llm_server 用 stub 上游验证隧道转发与
网关透传；harness 仅验证只读端点（`/health`、`/agents`、`/skills`）。
详见 [`e2e.md`](./e2e.md)。

---

## 4. 本地一键

```powershell
# 后端 harness：案例回归 + 单元（推荐日常使用）
cd server && cargo test -p harness

# 全链路 E2E（e2e_tests/run_full_chain_e2e.ps1）
cd tcm_work/e2e_tests && .\run_full_chain_e2e.ps1            # 默认不含 rrserver、不含前端
.\run_full_chain_e2e.ps1 -WithRrserver                       # 含 rrserver 隧道
.\run_full_chain_e2e.ps1 -WithFrontend                       # 含前端契约（需先对齐契约）
```

---

## 5. CI（建议）

| Job | 命令 | 门禁 |
|---|---|---|
| harness | `cd server && cargo test -p harness` | 通过（含 cases 回归） |
| rrserver | `cd server && cargo test -p rrserver` | 通过 |
| frontend | `npx vitest run` | 通过 |
| full-chain-e2e | `tcm_work/e2e_tests/run_full_chain_e2e.ps1 -SkipFrontend` | 通过 |

> 全链路 e2e 的 `llm_server`/`rrserver` 测试在对应产物或依赖缺失时自动 `skip`，
> 保证 CI 健壮性。需真实 LLM 的问诊链路（`/chat`）不在 CI 内，需连 LM Studio 手工验证。
