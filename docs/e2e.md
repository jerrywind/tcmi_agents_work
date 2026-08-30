# 全链路端到端测试（Full-Chain E2E）

覆盖 **rrserver → llm_server → harness** 跨组件链路，确保在「无真实 LM Studio / 无 GPU」
环境下也能跑通隧道与网关。套件位于 `tcm_work/e2e_tests/`（pytest + 前端 vitest），一键编排
`run_full_chain_e2e.ps1`。

> - 后端 **harness（Rust）** 的测试（含 `cases.jsonl` 案例回归）见 [`testing.md`](./testing.md)，
>   本文件只讲**跨组件**的全链路 e2e。
> - harness 的问诊链路（`/chat`）需真实 LLM，不在本套件内（无 mock 兜底）。

---

## 1. 结构

```
e2e_tests/
├── conftest.py                        # 各组件 base_url + 健康等待 + httpx fixtures
├── e2e_helpers.py                     # 共享辅助
├── test_rrserver_e2e.py               # rrserver 隧道：server+client 启动、token 鉴权、/t/<name> 转发
├── test_llm_server_e2e.py             # llm_server 网关：/healthz(degraded/ok)、/v1/models、chat 透传
├── _make_sample_image.py              # 生成 1x1 样例 JPEG（素材）
├── run_full_chain_e2e.ps1             # 一键编排：起 harness → pytest → 前端契约测试
├── run_manual_e2e.ps1                 # 人工验收：连真实 LLM 跑问诊并归档样例（T1.5）
└── images/                            # 运行期生成的样例图片（gitignore）

frontend/src/services/harness.contract.test.ts  # vitest：真实执行 harness.ts（Taro 适配层换成真实 fetch）
```

---

## 2. 分层设计

| 层 | 测试文件 | 验证点 | 依赖真实 LLM? |
|---|---|---|---|
| rrserver | `test_rrserver_e2e.py` | server/client 启动、token 鉴权、隧道把请求转发到本地 stub llm 并回传 | 否（stub 充当本地 llm） |
| llm_server | `test_llm_server_e2e.py` | 服务可达；无上游→`degraded` + `/v1/models` 503；有 stub 上游→`/v1/chat/completions` 透传 | 否（stub 充当 LM Studio） |
| harness | `run_full_chain_e2e.ps1` 启动后探活 | `/health` 可达（返回 `ok`） | 否（仅只读端点） |
| 前端→后端 | `frontend/src/services/harness.contract.test.ts` | 真实执行 `harness.ts` 函数：`/health`、`/agents`、`/skills`、`POST /skills` 错误分支，以及 MCP 的 `tools/list`、`list_agent_capabilities`（**默认开启**） | 否 |

> 前端契约用例会先探测 `/health`：后端不可达时整个 describe **自动 skip**，
> 因此无 Docker 或后端未起时 `npm run test` 依旧全绿，不会误报失败。

**离线跑通原理**：真实 LLM 推理依赖宿主机 LM Studio。全链路 e2e 通过
- llm_server / rrserver 用 **stub 上游**（Python 标准库 HTTP 服务）充当 LM Studio / 本地 llm，
验证网关透传与隧道转发能力；
- harness **不提供 MockProvider**，故只验证只读端点；问诊链路的确定性逻辑
（证候推断、配伍禁忌、方剂检索）由 Docker 内的 `cargo test -p harness --test cases` 覆盖。

各组件不可用时（缺 rrserver 二进制、缺 fastapi 依赖）测试会**自动 skip** 而非失败。

---

## 3. 运行

### 3.1 一键编排（推荐）
```powershell
cd tcm_work/e2e_tests
.\run_full_chain_e2e.ps1                       # harness(镜像) + pytest + 前端契约测试
.\run_full_chain_e2e.ps1 -WithRrserver         # 额外包含 rrserver 隧道（需 TCM_RRSERVER_BIN）
.\run_full_chain_e2e.ps1 -SkipFrontend         # 只跑 pytest
.\run_full_chain_e2e.ps1 -SkipBuild            # 复用已存在的镜像，不重新构建
```
脚本流程：生成样例图片 → `docker build` harness 镜像（多阶段，镜像内编译）
→ `docker run` 起容器（`:8011`）→ `/health` 探活 → 跑 pytest → 前端契约测试 → 删除容器。

> **后端完全依赖 Docker**：脚本不再使用 `target/{debug,release}/harness.exe`
> （宿主机 cargo 产物不作为交付/验证依据）。构建上下文为 workspace 根 `server/`。
> 常用开关：`-SkipBuild` 跳过构建、`-ImageName` 指定镜像名（默认 `tcm-harness:e2e`）。

### 3.2 分别运行
```powershell
# 跨组件 pytest（先手动起 harness 容器在 :8011）
$env:TCM_HARNESS_BASE = "http://127.0.0.1:8011"
cd e2e_tests
python -m pytest -q                        # 不含 rrserver
python -m pytest -q -k rrserver            # 只跑 rrserver

# 前端契约测试（先起 harness 在 :8011）
$env:VITE_API_BASE = "http://127.0.0.1:8011"
cd frontend && npx vitest run src/services/harness.contract.test.ts
```

### 3.3 人工端到端验收（T1.5，需真实 LLM）

自动化 e2e 只验证「链路通不通」，而**结论对不对必须由人看一眼**：

```powershell
cd tcm_work/e2e_tests
$env:HARNESS_LLM_API_KEY = '<LM Studio 令牌>'   # 若服务端开启了鉴权
.\run_manual_e2e.ps1 -Case damp-heat            # 或 wind-cold / red-flag
```

脚本会：Docker 起 harness（把 LM Studio 以 `host.docker.internal` 暴露给容器）
→ 用一段真实主诉跑完整个 `/chat` → 把原始响应、归档快照与
**人读版报告**写入 `docs/samples/<case>/` → 自动检查硬指标
（步骤齐全、主证非空、治疗有内容、红旗被拦、报告已落盘）。

自动检查只覆盖「有输出、结构完整」，**内容是否合理仍需人工审阅**
（证候是否对得上主诉、治疗是否安全可行）。三个用例：

| 用例 | 主诉要点 | 重点看什么 |
|---|---|---|
| `damp-heat` | 口苦口臭、大便粘滞、肢体困重、舌红苔黄腻 | 主证是否判为湿热类；方剂/调护是否对症 |
| `wind-cold` | 恶寒重发热轻、无汗、头身痛、脉浮紧 | 是否判为风寒表实；是否辛温解表 |
| `red-flag` | 突发胸痛、冷汗、呼吸困难 | **必须**被安全门拦截且不给治疗方案 |

> 前置：LM Studio 已启动并加载 `google/gemma-4-12b-qat`。
> 容器经 `host.docker.internal` 访问宿主机端口，Docker Desktop 上无需额外配置。

---

## 4. 关键环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `TCM_HARNESS_BASE` | `http://127.0.0.1:8011` | harness 地址（编排脚本自动设置） |
| `TCM_LLM_BASE` | `http://localhost:8000` | llm_server 网关地址（`conftest.py`） |
| `TCM_RRSERVER_SERVER_BASE` | `http://localhost:8088` | rrserver 云端中继 |
| `TCM_RRSERVER_CLIENT_BASE` | `http://localhost:9000` | rrserver 家庭端 client |
| `TCM_FRONTEND_BASE` | `http://localhost:10086` | 前端 H5 dev server |
| `TCM_RRSERVER_BIN` | — | rrserver 二进制路径（`-WithRrserver` 时用，缺省按 `server/rrserver/target/**` 查找） |
| `VITE_API_BASE` | `http://127.0.0.1:8011` | 前端契约测试指向的后端地址（编排脚本自动设置） |
| `TCM_E2E_HEALTH_TIMEOUT` / `TCM_E2E_HTTP_TIMEOUT` | `60` / `30` | 健康等待与请求超时（秒） |
- `HARNESS_LLM_BASE_URL` / `HARNESS_LLM_API_KEY` / `HARNESS_MODEL`：harness 连接 LLM 用
  （前缀是 `HARNESS_`；无 LLM 时仅只读端点可用）。
- `HARNESS_TUNNEL_SERVER` / `HARNESS_TUNNEL_NAME` / `HARNESS_TUNNEL_TOKEN`：harness 经
  rrserver 隧道暴露（等价于命令行 `--tunnel-*`）。
- `TCM_E2E_HEALTH_TIMEOUT` / `TCM_E2E_HTTP_TIMEOUT`：健康等待与请求超时（秒）。
- 前端契约测试只需 Node 18+ 全局 `fetch`/`FormData`/`Blob`（无需额外依赖）。

---

## 5. 说明
- rrserver 隧道测试需要 rrserver 二进制：`TCM_RRSERVER_BIN` 显式指定，或
  `server/rrserver/target/{debug,release}/rrserver[.exe]`；未找到时自动 `skip`
  并给出提示。后端只在 Docker 内构建，该用例属**可选**项，默认不跑。
- llm_server 测试需要 Python 环境已安装 `fastapi` 等依赖；缺失时自动 `skip`。
- 全链路 e2e 与后端单测是**互补**的两套：后者覆盖 harness 内部的确定性逻辑
  与 YAML 资源完整性（含案例回归），前者覆盖跨组件集成与前端契约对齐。
