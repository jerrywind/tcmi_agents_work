# 开发文档（Development）

面向开发者：本地开发环境搭建、启动、调试与常见问题。**部署/端口/配置细节见
[`deployment.md`](./deployment.md)；模型与降级事实见 [`llm_server.md`](./llm_server.md)。
本文件只描述开发流程，不重复这些事实。**

---

## 1. 环境要求（建议）
- Rust 1.75+（**后端 harness 与 rrserver 均需**，`server/` Cargo workspace）
- Python 3.11+（llm_server；可选，仅当使用 Python 网关时）
- Node.js 18+（frontend）
- LM Studio（接入真实 LLM 时才需要，默认 `http://localhost:11223/v1`）

> 后端已由 Python（原 `backend/`，归档于 `_useless/backend/`）重写为 Rust **harness**，
> 故 Python 不再是后端开发的必需项；`TCM_STORE`/Redis 等旧后端配置随之失效。

---

## 2. 仓库结构
```
tcm_work/
├── frontend/      Taro 多端（H5/微信小程序），仅产出静态 dist，不含 web 服务器
├── server/        Rust workspace：后端 + 隧道
│   ├── harness/      诊断编排（Rust 复刻原 backend），7× Sub-Agent
│   │   ├── src/          程序逻辑（agents / orchestrator / knowledge / skills / mcp / http）
│   │   ├── resources/    可改 YAML 数据（证候/问诊/方剂/调护/安全规则…）
│   │   └── cases.jsonl   案例回归基准（源自原 backend 真实病例）
│   └── rrserver/      Rust 反向隧道：server + client
├── llm_server/    纯 LM Studio 网关 + Agent 中间层（.py）
├── deploy/        统一 nginx（反代/TLS）配置与顶层编排，后端服务容器不含 nginx
│   ├── nginx/         frontend.conf（前端静态+SPA回退+反代）、rrserver.conf（/rr TLS反代）
│   ├── certs/         TLS 证书（fullchain.pem / privkey.pem）
│   └── docker-compose.yml  nginx + harness + rrserver 统一编排
├── docs/          文档（见 docs/README.md 索引）
├── scripts/       本地脚本（cleanup.ps1；run_e2e.ps1 等已随 backend 归档至 _useless/scripts）
├── e2e_tests/     全链路 E2E（harness→rrserver→llm_server）
└── _useless/      归档（含原 Python 实现 _useless/backend/），不参与构建
```

---

## 3. 后端本地启动（harness）
```bash
cd server
cargo run -p harness            # 调试运行（cwd 为 workspace 根时会用默认 resources 路径）
# 推荐：在 server/harness 下运行，确保 resources/ 相对路径可用
cd harness && cargo run -- --listen 127.0.0.1:8011
# 验证：http://127.0.0.1:8011/health 、/agents 、/skills
```
- `--listen` 指定监听；`--resources` 覆盖 YAML 资源目录。
- 改 YAML 后重启或 `POST /reload`（需开启 `hot_reload`）生效。
- **无 LLM 时**：只读端点可用，`/chat` 会失败（harness 无 MockProvider）；
  确定性逻辑改完请跑 `cargo test -p harness --test cases`。

---

## 4. 接入真实 LLM（开发态）
> 详细步骤与模型事实见 [`usage.md`](./usage.md) 与 [`llm_server.md`](./llm_server.md)。
> 概要：
> 1. LM Studio 加载 `google/gemma-4-12b-qat`，开启 Local Server（`:11223`）。
> 2. （可选）起 `llm_server` 网关（`python -m app.main`，`:8000`）。
> 3. harness 指向 LLM：
>    ```powershell
>    $env:HARNESS_LLM_BASE_URL="http://localhost:8000/v1"    # 经网关
>    # 或直连 LM Studio（默认值）：http://localhost:11223/v1
>    $env:HARNESS_LLM_API_KEY="<LM Studio 开启校验时必填>"
>    cd server/harness && ../target/debug/harness --listen 127.0.0.1:8011
>    ```
> 注意：环境变量前缀是 **`HARNESS_`**（不是旧的 `TCM_LLM_*`）。

---

## 5. rrserver（家庭算力上云，可选）
- 先在 WSL2/Linux 或本地 `cd server && cargo build --release -p rrserver`
  （见 [`deployment.md`](./deployment.md) 5.1）。
- `cd server/rrserver && .\start_rrserver.ps1` 一键起 server(`:8088`) + client(`:9000`)；
  client 把家庭端本地 LLM 暴露为 server 的 `/t/home/*`。
- harness 也可直接经隧道暴露：`harness --tunnel-server ws://<rrserver> --tunnel-name tcm`
  （详见 deployment 5.2.1）。
- 调试时可用 `curl https://rr.windblue.tech/healthz`（应 `ok`）探测隧道状态。

---

## 6. 前端本地启动
```bash
cd frontend
npm install
npm run dev:h5      # H5 http://localhost:10086（apiBase 见 config/dev.ts）
npm run dev:weapp   # 微信小程序（需微信开发者工具）
```
前端 service 层契约见 `src/services/api.ts`；跨端通过 Taro 适配（H5 走原生 fetch、小程序走 `wx.request`）。

---

## 7. 测试
- **后端 harness**：`cd server && cargo test -p harness` —— 含单元 + `cases.jsonl`
  案例回归（不依赖 LLM）—— [`testing.md`](./testing.md)。
- **rrserver**：`cd server && cargo test -p rrserver`（单元 + 集成）。
- **前端单测**：`frontend/`（vitest + jsdom）。
- **全链路 E2E**（harness→rrserver→llm_server）：`tcm_work/e2e_tests/`，一键
  `run_full_chain_e2e.ps1` —— 见 [`e2e.md`](./e2e.md)。
- 临时文件/日志清理规范见 [`cleanup.md`](./cleanup.md)。

---

## 8. 常见问题（FAQ）
| 现象 | 排查 |
|---|---|
| harness `/chat` 报 LLM 不可用 | 未设置 `HARNESS_LLM_BASE_URL`（默认 `http://localhost:11223/v1`）或 LM Studio 未启动。harness 无 MockProvider，真实推理必须配 LLM。 |
| 设了 `TCM_LLM_BASE_URL` 却不生效 | 变量名前缀错误：harness 读 **`HARNESS_`** 前缀（如 `HARNESS_LLM_BASE_URL`、`HARNESS_LLM_API_KEY`）。 |
| harness 启动时报资源加载失败 | cwd 不对：`resources/` 是相对路径，须在 `server/harness` 下运行，或用 `--resources` 指定。 |
| harness 隧道连不上（WS 404） | 直连 rrserver server 时 `external_ws_base` 不应带 `/rr` 前缀（`/rr` 由 nginx 添加）。 |
| llm_server `/healthz` = `degraded` | 上游 LM Studio 未开或 `LMSTUDIO_BASE_URL` 不通。属预期降级，非错误。 |
| llm_server `/v1/models` 返回 503 | 同上，上游不可达。启动 LM Studio 后恢复。 |
| rrserver nginx 反复重启 | 缺 `deploy/certs/fullchain.pem` + `privkey.pem`，用 WSL2 `openssl` 生成自签名证书（nginx 配置见 `deploy/nginx/rrserver.conf`）。 |
| rrserver 容器内 cargo build 失败 | 容器内网络损坏 crates.io 下载；需在 WSL2 编译后 COPY 二进制（详见 deployment 5.1）。 |
| 前端 dev 连不上后端 | 检查 `config/dev.ts` 的 `apiBase` / `process.env.TCM_API_BASE` 是否指向 harness 地址与端口（`:8011`，经 nginx 为 `/api`）。注意前端契约仍按旧 backend，需先对齐。 |
| 视觉识别无独立服务 | 视觉与文本共用 `google/gemma-4-12b-qat` 多模态端点，不单独部署视觉模型。 |
