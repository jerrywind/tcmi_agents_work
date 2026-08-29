# 测试体系（Testing）

覆盖单元、集成、案例回归与**全链路 E2E**。测试目标与质量门禁：

- **后端（harness + rrserver，Rust）测试全绿**：在 Docker 内编译并执行（见下）。
- 关键路径 0 回归：诊断编排流程、7 个 Sub-Agent、skills 注册与 owner 过滤、
  PPG 解析、十八反十九畏与妊娠禁忌校验、YAML 资源完整性、rrserver 隧道转发。
- 临时文件/日志命名与清理见 [`cleanup.md`](./cleanup.md)。

> **后端一律通过 Docker 验证**，不使用本地 `cargo build` 产物。
> 因此下面的后端命令都包在 `docker run` 里；runner / 本机无需安装 Rust 工具链。

---

## 1. 后端 harness（Rust）

| 层级 | 位置 | 说明 |
|---|---|---|
| 单元 | `server/harness/src/**` 的 `#[cfg(test)]` | PPG 解析、配伍禁忌校验、证候/方剂检索、关键词证据匹配、技能注册与 owner 过滤 |
| 集成 | `server/harness/tests/cases.rs` | **案例回归**：以 `cases.jsonl` 真实病例校验资源与纯函数链路（不依赖 LLM） |
| 集成 | `server/harness/tests/behavior.rs` | **行为回归**：红旗中断判定、技能归属（含 treatment 专属工具）、`owner` 过滤、埋点累加、`mcp_clients` 配置解析、`/chat` 响应契约、**结构化辨证（主证/兼证/置信度/矛盾证据）**、**MCP Server 端点**（均不依赖 LLM） |
| 评测 | `server/harness/tests/llm_eval.rs` | **LLM 质量评分**（T4.4）：以 `cases.jsonl` 跑真实辨证并自动评分；默认跳过，`HARNESS_EVAL=1` 才启用 |

当前 **后端 125 个用例全绿**（harness 25：behavior 22 + cases 2 + llm_eval 1；rrserver 100）。
| 隧道 | `server/rrserver/tests/integration.rs` | rrserver 端到端：注册鉴权、隧道转发、流式、CORS、断线重连、模型部署包装 |

```bash
cd server
cargo test                              # 整个 workspace（harness + rrserver）
cargo test -p harness                   # 仅 harness
cargo test -p harness --test cases      # 仅案例回归
cargo test -p rrserver                  # 仅 rrserver
```

### 1.1 案例回归（`--test cases`）

基准数据 `server/harness/cases.jsonl`（93 条真实会诊记录），校验：

1. **关键词证据匹配**：案例主诉 + 证据能命中 `keywords.yaml` 的证据标签。
2. **证候推断**：`infer_syndrome_slug` 返回的候选集覆盖案例期望证候（**支持兼证**，
   返回全部得分 > 0 的证候，按得分降序）。
3. **方剂/调护检索**：每个期望证候能从 `formulas.yaml` / `care.yaml` 检索到内容。
4. **YAML 资源完整性**：案例中出现的证候必须在 `syndromes.yaml` 存在——
   缺数据时测试**直接失败并列出缺失证候**，是资源维护的回归护栏。

该测试**不依赖 LLM**（纯函数 + 资源数据），可在 CI 中稳定运行。
新增/调整 YAML 资源后跑它，即可确认未破坏既有病例。

> 需真实 LLM 的问诊链路（`/chat`）不在自动化测试内：harness 未提供 MockProvider，
> 请连 LM Studio 后手工验证，或用下面的 LLM 评测集半自动打分。

### 1.2 LLM 质量评测（`--test llm_eval`，T4.4）

```bash
# 默认跳过（打印提示后直接返回），因此不影响 CI 与日常 cargo test
docker run --rm -v "${PWD}/server:/build" -w /build \
  -e HARNESS_EVAL=1 \
  -e HARNESS_LLM_BASE_URL=http://host.docker.internal:11223/v1 \
  rust:1.98-bookworm \
  cargo test -p harness --test llm_eval -- --nocapture
```

- 数据来源：`cases.jsonl` 中带期望证候的病例，**按语料去重**（原始文件里同一主诉
  重复几十条，不去重会把评分带偏）；
- 评分：期望证候名出现在辨证输出中即命中，按病例给部分分，另统计全中率；
- 可调：`HARNESS_EVAL_LIMIT`（条数，默认 20）、`HARNESS_EVAL_TIMEOUT_SECS`
  （单条超时，默认 120，超时计 0 分而非挂住整轮）、`HARNESS_EVAL_MIN_SCORE`
  （总分门槛，默认 0 = 只出报告）；
- 产物：JSON 报告写到 `server/target/tmp/llm_eval_report.json`；
- 定位：**不作为 PR 门禁**（耗时与成本不可控、结果有随机性），
  由 `.github/workflows/llm-eval.yml` 每晚跑，跑在能访问本地 LLM 的 self-hosted runner 上。

---

## 2. 前端（vitest）

```bash
cd frontend
npm install
npx vitest run            # 单测（jsdom）
```

- 页面组件（`src/pages/**`）依赖 Taro 运行时，由契约测试与真机验证覆盖，
  不计入单测覆盖率；可独立验证的纯逻辑（如证候摘要格式化）抽到 `src/utils/`。
- 组件/逻辑单测用 `vitest + jsdom`，`@tarojs/taro` 由 `vitest.setup.ts` 全局 mock。
- 测试文件：`src/services/harness.test.ts`（契约客户端）、
  `src/services/harness.contract.test.ts`（连真实 harness，不可达自动 skip）、
  `src/utils/format.test.ts`。
- 当前 **29 个用例全绿**（含 6 条契约：`/health`、`/agents`、`/skills`、
  `POST /skills` 错误分支 + MCP 的 `tools/list`、`list_agent_capabilities`）。

> 2026-08-29 修复：此前 18 个用例失败，根因是 `vitest.setup.ts` 里的
> `@tarojs/taro` mock 返回的是**普通 async 函数**而非 `vi.fn()`，
> 导致 `vi.mocked(Taro.request).mockResolvedValue(...)` 报
> `TypeError: mockImplementation is not a function`；
> 同时 `vitest.config.ts` 把后端地址硬编码成已废弃的 `:22000`（旧 Python backend 端口）。
> 现在 mock 用 `vi.hoisted()` + `vi.fn()` 构造，端口指向 harness 的 `:8011`。

---

## 3. 全链路 E2E（rrserver→llm_server→harness）

独立套件位于 `tcm_work/e2e_tests/`，一键编排 `run_full_chain_e2e.ps1`：

- `test_rrserver_e2e.py`：rrserver 隧道转发（需 rrserver 二进制，缺失则 skip）
- `test_llm_server_e2e.py`：llm_server 网关健康检查与透传（无上游→`degraded`/`503`）
- 前端 `frontend/src/services/harness.contract.test.ts`（vitest，**默认开启**：
  连真实 harness 校验 `/health`、`/agents`、`/skills`、`POST /skills` 错误分支；
  后端不可达时自动 skip，`-SkipFrontend` 可关闭）

编排脚本用 `docker build` + `docker run` 起 harness（后端完全依赖 Docker，
不使用宿主机 cargo 产物），详见 [`e2e.md`](./e2e.md)。

设计要点：**无真实 LLM 也能跑通**——rrserver/llm_server 用 stub 上游验证隧道转发与
网关透传；harness 仅验证只读端点（`/health`、`/agents`、`/skills`）。
详见 [`e2e.md`](./e2e.md)。

---

## 4. 本地一键

```powershell
# 后端 harness + rrserver：单测 + 案例回归（在 Docker 内编译执行）
cd tcm_work
docker run --rm -v "${PWD}/server:/build" -w /build rust:1.98-bookworm `
  cargo test --workspace

# 后端 lint：fmt + clippy 严格门禁
docker run --rm -v "${PWD}/server:/build" -w /build rust:1.98-bookworm `
  bash -c "rustup component add rustfmt clippy && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings"

# 后端镜像（多阶段，镜像内编译）—— 等价于 scripts/build-release.ps1
pwsh scripts\build-release.ps1
```

> 挂载 `server:/build` 时容器内会重新编译；依赖已由 Cargo 缓存，
> 但 Docker 每次都是新容器，首次运行较慢（约 1~3 分钟）。
> CI 中同样用这两个 `docker run` 命令（见 `.github/workflows/test.yml`）。

---

## 5. CI（建议）

| Job | 做什么 | 门禁 |
|---|---|---|
| `backend-test` | Docker 内 `cargo test --workspace` | 通过（含 93 条案例回归） |
| `backend-lint` | Docker 内 `cargo fmt --check` + `clippy -D warnings` | 无告警 |
| `backend-image` | `docker build` harness / rrserver（镜像内编译） | 构建成功 |
| `frontend` | `npm ci && npm run test` | 通过（契约测试自动 skip） |
| `frontend-contract` | 起真实 harness 容器后跑契约测试 | 通过 |

**nightly（`.github/workflows/llm-eval.yml`，非 PR 门禁）**

| Job | 做什么 | 门禁 |
|---|---|---|
| `eval` | self-hosted runner 上连真实 LLM 跑 `llm_eval` | 报告存档；`HARNESS_EVAL_MIN_SCORE` 设门槛后才判失败 |

> 全链路 e2e 的 `llm_server`/`rrserver` 测试在对应产物或依赖缺失时自动 `skip`，
> 保证 CI 健壮性。需真实 LLM 的问诊链路（`/chat`）：PR 门禁内不跑，
> 改为 nightly 评测集打分（见 1.2）。
