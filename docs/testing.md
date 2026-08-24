# 测试与质量门禁

本项目对后端（Python/FastAPI）与前端（Taro/TypeScript）均建立了工程化测试体系，并以覆盖率作为质量门禁。

## 后端测试（pytest）

### 依赖
开发/CI 依赖见 `backend/requirements-dev.txt`：

```bash
cd backend
pip install -r requirements-dev.txt
```

### 运行
```bash
cd backend
python -m pytest                 # 全部用例
python -m pytest -q --cov=app    # 带覆盖率（term 报告）
python -m pytest --cov=app --cov-report=html:htmlcov   # 生成 HTML 报告
```

### 目录结构
`backend/tests/` 下按职责分文件：

| 文件 | 覆盖内容 |
| --- | --- |
| `test_config.py` | 配置默认值与环境变量覆盖 |
| `test_schemas.py` | 数据模型校验、字面量约束 |
| `test_protocol_registry.py` | 能力枚举、`AgentRequest/Response` 信封、agent 解析与切换（含重复注册、缺失 impl 兜底、KeyError 防御分支） |
| `test_store.py` | `MemoryStore` 与 `RedisStore`（内存桩，无需真实 Redis） |
| `test_knowledge.py` | 证候库、问诊题库、方案库、红旗词健全性 |
| `test_agents.py` | 各 sub-agent 的 rule 与 llm(回退) 两条路径 |
| `test_llm_agents.py` | 通过 FakeProvider 覆盖 LLM 成功分支、provider 工厂，及 `OpenAICompatProvider.chat` 真实调用路径 |
| `test_orchestrator.py` | 状态机单元方法 + 端到端多轮对话 |
| `test_main.py` | FastAPI 接口（TestClient）覆盖健康/会话/启动/问答/上传/报告 |
| `test_mcp_server.py` | MCP Server：两层工具清单与 schema、7 项 Agent 级能力工具、`run_agent` 通用入口与自省、异常兜底、HTTP 端点可重入 |
| `test_mcp_client.py` | MCP Client：`MCPToolHub` 生命周期与容错、结果归一化、外部工具注册/卸载、远程 Sub-Agent 桥（转发、自定义工具名、四类降级） |
| `test_mcp_api.py` | MCP 管理端点（`/api/mcp/*`）、`/mcp` 挂载与 `initialize` 握手、`mcp` 段配置与环境变量覆盖 |
| `e2e/` | **端到端测试**：启动真实 uvicorn 服务 + httpx 直连（见下） |

### 端到端测试（E2E）
`tests/e2e/` 通过 `conftest.py` 在守护线程中启动**真实 uvicorn 服务**（自动选取空闲端口），
再用 `httpx` 直连 `127.0.0.1` 真实 socket，验证「部署态」行为，而非仅路由逻辑：

- 服务启动 / 健康检查 / `/openapi.json` / `/docs` 可达；
- CORS：跨域实际请求与 `OPTIONS` 预检均返回正确头；
- 完整问诊生命周期（新建 → 启动 → 多轮问答 → 出报告 → trace 轨迹）；
- 红旗症状中断问诊并转诊；
- 图片上传（multipart）后经真实静态挂载 `/uploads/` 可取回；
- 系统路由可观测性（`GET /api/system/agents`）、SKILL 工具按 capability 暴露（如 `treatment.plan`）；
- SKILL 运行时生命周期：`GET /api/skills` 列举、按路径/名称热装载、卸载、错误路径（400/404）。

服务运行在 pytest 同进程线程内，故覆盖率仍可统计；用空闲端口 + 就绪轮询（含重试）规避端口竞态。

```bash
cd backend
python -m pytest tests/e2e -q                 # 仅 E2E
python -m pytest -m e2e -q                     # 等价（marker）
python -m pytest -q                            # 单元 + E2E 一并执行（CI 默认）
# 直连已部署/容器化后端（不本进程启动 uvicorn），复用全部 E2E 用例：
E2E_BASE_URL=http://127.0.0.1:8000 python -m pytest -m e2e -q
```

### 设计要点
- 异步用例使用 `pytest-asyncio`（`pytest.ini` 中 `asyncio_mode = auto`）。
- LLM 实现在无 API Key 时回退到 `MockProvider`，保证无网络也可完整跑通。
- `RedisStore` 通过内存 Redis 桩注入测试，不依赖外部服务。
- 默认 rule 实现路径被完整覆盖；LLM 路径通过 `FakeProvider` 注入 JSON 覆盖成功分支。
- E2E 复用真实 `app` 与默认（rule）路由，无需外部依赖即可跑通辨证→方案全链路。

## 前端测试（vitest）

### 依赖
开发依赖已写入 `frontend/package.json`（`vitest` / `jsdom` / `@vitest/coverage-v8`）。仓库根已放置 `frontend/.npmrc`（`legacy-peer-deps=true`），用于绕开 `vitest`(vite@5) 与 `@tarojs/plugin-framework-react`(vite@4) 的 peer 冲突，首次安装直接执行：

```bash
cd frontend
npm install
```

### 运行
```bash
cd frontend
npm run test          # 单次运行
npm run test:watch    # 监听模式
npm run test:cov      # 带覆盖率（text + html + lcov）
```

### 目录结构
- `src/utils/format.ts` + `src/utils/format.test.ts`：纯工具（置信度百分比、类别色标、状态判断）。
- `src/services/api.ts` + `src/services/api.test.ts`：API 封装，使用 `vi.mock('@tarojs/taro')` 桩替身验证请求/错误/上传逻辑。
- `src/services/api.skills.test.ts`：SKILL 管理端点（get/load/unload）的客户端封装验证。
- `src/services/api.contract.test.ts`：**前端↔后端契约测试**（见下）。
- `vitest.config.ts` / `vitest.setup.ts`：jsdom 环境、覆盖率配置、Taro 全局桩。

### 前端↔后端契约测试
`src/services/api.contract.test.ts` 用**真实 Node `fetch` 替身替换 `Taro.request` / `Taro.uploadFile`**，
让 `api.ts` 客户端直连真实后端服务，校验请求/响应契约（字段、状态码、错误形态），
验证前端客户端与后端 API 的一致性：

- 覆盖 `createConsultation` / `startConsultation` / `answerQuestion` / `getState` /
  `getSkills` / `loadSkill` / `unloadSkill` / `uploadImage` 全链路；
- 使用 `// @vitest-environment node`，保证 `fetch` / `FormData` / `Blob` 为真实 Node 实现，
  避免 jsdom 下的 `FormData` 与 Node `fetch` 不兼容导致 multipart 体为空；
- 通过顶层 `await` 探测健康端点，**后端不可达时整个 `describe` 自动 skip**，
  因此无后端环境（如纯前端 CI）下 `npm run test` 仍全绿；本地起后端后自动执行。

```bash
cd frontend
npm run test                       # 含契约测试（无后端时自动跳过）
VITE_API_BASE=http://127.0.0.1:8000 npm run test   # 指向指定后端地址
```

### 设计要点
- 小程序运行时 `@tarojs/taro` 在 Node 不可用，由 `vitest.setup.ts` 全局桩替身；
  契约测试则在运行时把该桩替身重定向到真实 HTTP，从而直连后端。
- 仅抽取“可独立验证”的纯逻辑（工具函数、API 封装）做单测，组件渲染依赖 Taro 运行时，交由端到端/真机验证。
- 契约测试不修改 `api.ts` 业务，仅替换传输层，能在 CI 中由独立 job 启动真实后端实跑验证。

## 覆盖率目标
- 后端：`pytest --cov=app`，核心业务（agents / orchestrator / 协议）目标 ≥ 90%；当前实测约 **97%**（单元 + E2E 一并执行），其中 `protocol/llm.py`、`protocol/registry.py`、`skills/toolcall.py` 等均 100% 覆盖。
- 前端：聚焦可测逻辑（utils + services）的语句/分支覆盖；当前实测 100% 语句/行/函数、约 96% 分支（页面组件排除在单元覆盖率基数外，由端到端/真机验证）。

> 前端端到端（小程序真机 / 微信开发者工具）目前依赖 Taro 运行时，未纳入单元覆盖率基数；建议接入方在真机或模拟器中按 `docs/usage.md` 的流程做人工/自动化 E2E 验收。

## CI
`.github/workflows/test.yml` 在推送/PR 时并行执行 4 个 job，任一失败即阻断合并：

- `backend`：后端单元/集成（`--ignore=tests/e2e`），并设 **覆盖率强制门禁 `--cov-fail-under=90`**（实测约 96%，低于阈值即阻断合并）。
- `e2e`：后端端到端（真实 uvicorn 服务，`-m e2e`，独立 job 便于单独超时与日志隔离）。
- `e2e-docker`：后端端到端（**容器化部署态**）——`docker compose up --build` 拉起真实镜像，复用同一批 E2E 用例（经 `E2E_BASE_URL` 直连容器）验证可部署产物，结束后 `docker compose down -v`。
- `frontend`：前端单元（vitest，契约测试在无后端时自动 skip）。
- `contract`：前端↔后端契约测试（单独启动真实后端并实跑 `api.contract.test.ts`）。

工作流顶层配置 `concurrency`（同分支/PR 取消进行中旧运行），每个 job 设 `timeout-minutes` 防止挂死。

### 本地一键端到端
仓库提供 `scripts/run_e2e.ps1`（PowerShell，适配 Windows 开发机）：自动启动后端（若 8000 已被占用则复用）、轮询就绪、依次跑后端 E2E 与前端契约测试、结束后关闭本次启动的后端。

```powershell
pwsh -ExecutionPolicy Bypass -File scripts/run_e2e.ps1
```

> 脚本会在 `backend/` 下生成 `uvicorn_e2e.log` / `uvicorn_e2e_err.log`，已被 `.gitignore` 忽略；其产生的 `htmlcov/` 同样忽略。

### 临时文件与清理规范

测试/验证过程中产生的临时脚本、探针、调试产物**必须遵循统一命名约定**，以便被自动清理且不污染源码分析（pyright/pytest 会误收集源码树内的临时 `.py`）。

**命名约束（详见 [`docs/cleanup-rules.md`](./cleanup-rules.md)）：**

- 临时验证/测试/草稿脚本或报告，文件名必须以 `_verify_` / `_tmp_` / `_gen_` / `_scratch_` 之一开头（如 `_verify_responses.py`、`_tmp_probe.py`）；
- 调试遗留文件固定名：`mcp_check.txt`、`_pyr*.json`、`one.txt`、`cov_unit.txt`、`pytest_out.txt`；
- **禁止**在 `backend/app/` 源码树内创建临时 `.py`，**禁止**与源码同名（如 `agent.py`、`store.py`）；
- 验证结束后立即删除；忘记删除的由 `scripts/cleanup.ps1` 兜底。

**统一清理（默认仅预览，加 `-Clean` 才删除，且永不删除 git 跟踪文件）：**

```powershell
pwsh scripts/cleanup.ps1          # 预览待清理项
pwsh scripts/cleanup.ps1 -Clean   # 实际删除（忽略产物 + 命名约定的临时文件）
```

> 运行/测试产生的日志（`run.log`、`uvicorn*.log`、`*.err`、`rrserver.log`、`htmlcov/` 等）均已被 `.gitignore` 忽略，属于可清理范围；若需保留某次日志排障，请重命名为带日期的归档名并移出仓库根。
