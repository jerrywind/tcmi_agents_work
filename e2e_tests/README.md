# TCM 全链路端到端测试（e2e_tests）

覆盖 **前端 → 后端 → rrserver → llm_server** 全链路，验证在「无真实 LM Studio / 无 GPU」环境下也能跑通。

## 目录结构
```
e2e_tests/
├── conftest.py                        # 公共配置：各组件 base_url + 健康等待工具 + httpx fixtures
├── e2e_helpers.py                     # 驱动问诊 / 读取产物 的共享辅助
├── test_rrserver_e2e.py               # rrserver 反向隧道：server+client 启动、register 鉴权、/t/<name> 隧道转发
├── test_llm_server_e2e.py            # llm_server 网关：/healthz(degraded/ok)、/v1/models、/v1/chat/completions 透传
├── test_backend_llm_integration_e2e.py # backend ↔ llm_server：配置接入网关 + mock 驱动问诊到 finished + report/evidences/trace/图片
├── _make_sample_image.py             # 生成 1x1 样例 JPEG（上传测试素材）
├── run_full_chain_e2e.ps1            # 一键编排：起 backend(mock) → 跑 pytest → 跑前端 vitest
└── images/sample.jpg                 # 自动生成的样例图片
```

前端侧另有函数级 e2e（真实执行 `src/services/api.ts`）：
```
frontend/src/services/api.e2e.test.ts  # vitest：把 Taro 适配层替换为真实 fetch 到 backend
```

## 分层设计
| 层 | 测试文件 | 验证点 | 是否依赖真实 LLM |
|---|---|---|---|
| rrserver | `test_rrserver_e2e.py` | server/client 启动、token 鉴权、隧道把请求转发到本地 stub llm 并回传 | 否（用 stub 充当本地 llm） |
| llm_server | `test_llm_server_e2e.py` | 服务可达；无上游→`degraded` + `/v1/models` 503；有 stub 上游→`/v1/chat/completions` 透传 | 否（用 stub 充当 LM Studio） |
| backend ↔ llm_server | `test_backend_llm_integration_e2e.py` | backend 配置接入网关不崩溃；mock 驱动问诊到 finished，产出 report/evidences/trace，图片上传读取 | 否（用 mock 模式） |
| 前端 → 后端 | `frontend/src/services/api.e2e.test.ts` | 真实执行 `api.ts` 函数，从前端视角跑通完整问诊并拿到产物 | 否（backend mock 模式） |

## 运行

### 方式一：一键编排（推荐）
进入仓库根目录 `tcm_work/`，在 `e2e_tests/` 下执行 PowerShell：
```powershell
cd tcm_work/e2e_tests
# 跑 llm_server + backend + 前端（默认不含 rrserver，因其需 Rust 编译产物）
.\run_full_chain_e2e.ps1

# 额外包含 rrserver 隧道测试（需先 cargo build rrserver）
.\run_full_chain_e2e.ps1 -WithRrserver

# 跳过前端
.\run_full_chain_e2e.ps1 -SkipFrontend
```
脚本会：生成样例图片 → 启动 backend（自动降级 MockProvider，无需真实 LLM） → 跑 pytest 三层 → 跑前端 vitest → 关闭进程。

### 方式二：分别运行
```powershell
# 后端三层（需先手动起 backend 在 :8001，TCM_LLM_BASE_URL 留空以自动降级）
$env:TCM_BACKEND_BASE = "http://127.0.0.1:8001"
python -m pytest e2e_tests -q

# 前端 e2e（需先起 backend 在 :8000，TCM_LLM_BASE_URL 留空以自动降级）
$env:TCM_API_BASE = "http://localhost:8000"
cd frontend && npx vitest run src/services/api.e2e.test.ts
```

## 关键环境变量
- `TCM_BACKEND_BASE` / `TCM_LLM_BASE` / `TCM_RRSERVER_SERVER_BASE` / `TCM_RRSERVER_CLIENT_BASE`：各组件地址（conftest 默认值见文件）。
- `TCM_BACKEND_LLM_BASE`：backend 集成测试中指向的 LLM 网关地址。
- `TCM_API_BASE`：前端 e2e 指向的 backend 地址。
- `TCM_E2E_HEALTH_TIMEOUT` / `TCM_E2E_HTTP_TIMEOUT`：健康等待与请求超时（秒）。

## 说明
- 真实 LLM 推理依赖宿主机的 LM Studio（默认 `http://localhost:11223/v1`）。全链路 e2e 通过 **backend 自动降级 MockProvider**（不设 `TCM_LLM_BASE_URL` → 规则兜底）与 **stub 上游** 实现离线跑通，独立于 GPU/模型验证各组件的状态机、产物生成与契约一致性。
- rrserver 隧道测试需要本地已编译的 `rrserver` 二进制（`target/debug/rrserver` 或 `target/release/rrserver`）；未编译时自动 `skip` 并给出构建提示。
- llm_server 测试需要 Python 环境已安装 `fastapi` 等依赖；缺失时自动 `skip`。
