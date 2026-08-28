# 全链路端到端测试（Full-Chain E2E）

覆盖 **rrserver → llm_server → harness** 跨组件链路，确保在「无真实 LM Studio / 无 GPU」
环境下也能跑通隧道与网关。套件位于 `tcm_work/e2e_tests/`（pytest + 前端 vitest），一键编排
`run_full_chain_e2e.ps1`。

> - 后端 **harness（Rust）** 的测试（含 `cases.jsonl` 案例回归）见 [`testing.md`](./testing.md)，
>   本文件只讲**跨组件**的全链路 e2e。
> - 原 backend（Python）的契约用例 `test_backend_llm_integration_e2e.py` 已随 backend 归档，
>   默认排除；harness 的问诊链路需真实 LLM，不在本套件内。

---

## 1. 结构

```
tcm_work/e2e_tests/
├── conftest.py                        # 各组件 base_url + 健康等待 + httpx fixtures
├── e2e_helpers.py                     # 驱动问诊 / 读取产物 的共享辅助
├── test_rrserver_e2e.py               # rrserver 隧道：server+client 启动、token 鉴权、/t/<name> 转发
├── test_llm_server_e2e.py            # llm_server 网关：/healthz(degraded/ok)、/v1/models、chat 透传
├── test_backend_llm_integration_e2e.py # [已归档] 原 backend(Python) 契约用例，默认排除
├── _make_sample_image.py              # 生成 1x1 样例 JPEG（上传素材）
├── run_full_chain_e2e.ps1            # 一键编排：起 harness → pytest → 前端 vitest(默认跳过)
└── images/sample.jpg                  # 自动生成的样例图片

frontend/src/services/api.e2e.test.ts  # vitest：真实执行 api.ts（Taro 适配层替换为真实 fetch）
```

---

## 2. 分层设计

| 层 | 测试文件 | 验证点 | 依赖真实 LLM? |
|---|---|---|---|
| rrserver | `test_rrserver_e2e.py` | server/client 启动、token 鉴权、隧道把请求转发到本地 stub llm 并回传 | 否（stub 充当本地 llm） |
| llm_server | `test_llm_server_e2e.py` | 服务可达；无上游→`degraded` + `/v1/models` 503；有 stub 上游→`/v1/chat/completions` 透传 | 否（stub 充当 LM Studio） |
| harness | `run_full_chain_e2e.ps1` 启动后探活 | `/health` 可达；`/agents`、`/skills` 返回 7 个 agent 与 9 个技能 | 否（仅只读端点） |
| 前端→后端 | `frontend/src/services/api.e2e.test.ts` | 真实执行 `api.ts` 函数（**默认跳过**：契约未对齐，需 `-WithFrontend`） | 否 |

**离线跑通原理**：真实 LLM 推理依赖宿主机 LM Studio。全链路 e2e 通过
- llm_server / rrserver 用 **stub 上游**（Python 标准库 HTTP 服务）充当 LM Studio / 本地 llm，
验证网关透传与隧道转发能力；
- harness **不提供 MockProvider**，故只验证只读端点；问诊链路的确定性逻辑
（证候推断、配伍禁忌、方剂检索）由 `cargo test -p harness --test cases` 覆盖。

各组件不可用时（缺 Rust 二进制、缺 fastapi 依赖）测试会**自动 skip** 而非失败。

---

## 3. 运行

### 3.1 一键编排（推荐）
```powershell
cd tcm_work/e2e_tests
.\run_full_chain_e2e.ps1                       # 跑 harness 探活 + pytest（不含 rrserver、不含前端）
.\run_full_chain_e2e.ps1 -WithRrserver         # 额外包含 rrserver 隧道（需先 cargo build rrserver）
.\run_full_chain_e2e.ps1 -WithFrontend         # 额外跑前端 vitest（需先对齐前端契约）
```
脚本流程：生成样例图片 → 启动 harness（`server/harness/target/{debug,release}/harness.exe`，
`:8011`，cwd 为 `server/harness` 以定位 `resources/`）→ `/health` 探活 → 跑 pytest →
（可选）前端 vitest → 关闭进程。

> harness 需先构建：`cd server && cargo build -p harness`（或 `--release`）。

### 3.2 分别运行
```powershell
# 跨组件 pytest（先手动起 harness 在 :8011）
$env:TCM_HARNESS_BASE="http://127.0.0.1:8011"
python -m pytest tcm_work/e2e_tests -q -k "not backend"

# 前端 e2e（需先对齐前端契约；先起 harness 在 :8011）
$env:TCM_API_BASE="http://localhost:8011"
cd frontend && npx vitest run src/services/api.e2e.test.ts
```

---

## 4. 关键环境变量
- `TCM_HARNESS_BASE`：harness 地址（默认 `http://127.0.0.1:8011`）。
- `TCM_LLM_BASE` / `TCM_RRSERVER_SERVER_BASE` / `TCM_RRSERVER_CLIENT_BASE`：各组件地址（conftest 默认值见文件）。
- `TCM_BACKEND_LLM_BASE`：llm_server 相关用例指向的 LLM 网关地址。
- `TCM_API_BASE`：前端 e2e 指向的后端地址（对齐契约后为 harness `:8011`）。
- `HARNESS_LLM_BASE_URL` / `HARNESS_LLM_API_KEY` / `HARNESS_MODEL`：harness 连接 LLM 用
  （前缀是 `HARNESS_`；无 LLM 时仅只读端点可用）。
- `HARNESS_TUNNEL_SERVER` / `HARNESS_TUNNEL_NAME` / `HARNESS_TUNNEL_TOKEN`：harness 经
  rrserver 隧道暴露（等价于命令行 `--tunnel-*`）。
- `TCM_E2E_HEALTH_TIMEOUT` / `TCM_E2E_HTTP_TIMEOUT`：健康等待与请求超时（秒）。
- 前端 e2e 还需 `TCM_API_BASE` 与 Node 18+ 全局 `fetch`/`FormData`/`Blob`（无需额外依赖）。

---

## 5. 说明
- rrserver 隧道测试需要本地已编译的 `rrserver` 二进制（`server/target/debug/rrserver` 或
  `server/target/release/rrserver`）；未编译时自动 `skip` 并给出构建提示
  （WSL2/Linux `cargo build --release`）。
- llm_server 测试需要 Python 环境已安装 `fastapi` 等依赖；缺失时自动 `skip`。
- 全链路 e2e 与 `cargo test -p harness` 是**互补**的两套：后者覆盖 harness 内部的确定性逻辑
  与 YAML 资源完整性（含案例回归），前者覆盖跨组件集成与前端契约对齐。
