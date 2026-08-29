# 部署文档（Deployment）

覆盖四组件的部署：**前端 / harness / llm_server / rrserver**，以及端口、配置、网络与全链路验证。

> 后端为 Rust **harness**（`server/harness`），与 rrserver 同属 `server/` Cargo workspace，
> 统一用 `cargo build` 构建。

> 模型与降级事实的权威说明见 [`llm_server.md`](./llm_server.md)：`llm_server` 是纯 LM Studio
> 网关（**不托管模型**），模型统一 `google/gemma-4-12b-qat`（文本+视觉共用，原生多模态），
> LM Studio 默认 `http://localhost:11223/v1`。无上游时 llm_server `degraded`/`503`；
> harness **无 MockProvider**，只读端点（`/health`、`/agents`、`/skills`）仍可用，
> 但问诊推进（`/chat`）需真实 LLM。

---

## 1. 端口与地址（单一事实源）

| 组件 | 容器/进程端口 | 对外/说明 |
|---|---|---|
| 前端（dev） | 10086 | `npm run dev:h5` |
| 前端（build，nginx） | 8080 | 生产静态产物 |
| harness | 8011 | `/chat`、`/agents`、`/skills`、`/health`、`/reload`（经 nginx 以 `/api` 前缀对外） |
| llm_server | 8000 | `/healthz`、`/v1/*`（网关） |
| LM Studio（宿主机） | 11223 | `http://localhost:11223/v1` |
| rrserver server | 8088（deploy nginx→容器 8080） | `/healthz`、`/api/register`、WS `/ws/<name>`、隧道 `/t/<name>/*` |
| rrserver client | 9000 | 家庭端隧道客户端（连 server 的 WS，转发 `--local`） |
| RAG（可选） | 8080（默认 `RAG_PORT`） | `llm_server/rag`，详见 `rag.md` |

> 注意：rrserver 容器内监听 `8080`，经 `deploy/` 的统一 nginx 的 `8088` 对外；client 监听 `9000`。
> 与早期文档的「server:8080 直连」不同，当前统一以 `8088` 为对外入口（由 `deploy/nginx/rrserver.conf` 提供）。

---

## 2. 前端部署

### 2.1 开发（H5）
```bash
cd frontend && npm install && npm run dev:h5   # http://localhost:10086
```
`apiBase` 由 `config/dev.ts`（或 `process.env.TCM_API_BASE`）指定，指向 harness。

### 2.2 生产（H5 静态产物 + 统一 nginx）
```bash
cd frontend && npm run build:h5     # 产物在 dist/
# 静态托管与反向代理统一由 deploy/ 的 nginx 完成（见 deploy/docker-compose.yml）：
#   - deploy/nginx/frontend.conf：托管 dist/（SPA 回退）+ 反代 /api 到 harness（剥离前缀）
#     （harness 不落盘图片，无 /uploads 目录，故不再反代该路径）
#   - deploy/nginx/rrserver.conf：TLS 终止 + 反代 /rr 到 rrserver
# 启动：docker compose -f deploy/docker-compose.yml up -d --build
```
微信小程序：微信开发者工具导入 `frontend/`，走小程序审核发布流程。

---

## 3. harness 部署（Rust 后端）

### 3.1 本地
```bash
cd server
cargo build --release                       # 产物 server/target/release/harness(.exe)
cd harness
../target/release/harness --listen 0.0.0.0:8011
```
> **cwd 要点**：harness 默认按相对路径 `resources/` 加载 YAML（可用 `--resources` 覆盖），
> 故须在 `server/harness` 目录下运行，否则资源加载失败。

### 3.2 Docker Compose（与前端/rrserver 统一编排）
```bash
docker compose -f deploy/docker-compose.yml up -d --build   # nginx + harness + rrserver
```
`deploy/docker-compose.yml` 中 harness 服务：
- 构建上下文 `../server/harness`，容器内监听 `0.0.0.0:8011`；
- 挂载 `../server/harness/resources:/data/resources:ro`——**改 YAML 无需重建镜像**；
- nginx 的 `frontend.conf` 把 `/api/` 剥离前缀后转发到 `harness:8011`
  （harness 自身端点无 `/api` 前缀）。

> 容器镜像打包的是 **WSL2 预编译二进制**（容器内 `cargo build` 会因网络损坏 crates.io
> 下载而失败）；详见 `server/harness/Dockerfile`。

#### 构建前置：先编译 release 二进制

```powershell
# 推荐：在 WSL 原生目录编译（快，约 90s），完成后自动拷回 server/target/release/
powershell -NoProfile -File scripts\build-release.ps1
# 磁盘空间不足时可改为就地编译（明显更慢）：... -InPlace
```

> **为什么构建上下文必须是 `server/`（workspace 根）**
> harness 与 rrserver 同属一个 Cargo workspace，`cargo build --release` 的产物统一落在
> **`server/target/release/`**，子 crate 没有独立 `target/`。因此 compose 里写的是
> `context: ../server` + `dockerfile: harness/Dockerfile`，Dockerfile 内则 COPY
> `target/release/harness`。若把上下文设成 `server/harness`，COPY 会因找不到文件而失败。
>
> 由于 `server/target/` 常达 **数 GB**，上下文裁剪由 `server/.dockerignore` 负责
> （白名单策略：只放行两个二进制、YAML 资源与两个 Dockerfile）。
> 生效时构建上下文约 **20 MB**；若该文件被误删，每次构建都会传输整个 target。

### 3.3 关键配置项

配置优先级（低→高）：`resources/config.yaml` → 环境变量 `HARNESS_*` → 命令行参数。

**命令行参数**（仅 3 类，其余走环境变量/配置文件）：

| 参数 | 默认 | 说明 |
|---|---|---|
| `--config` | `resources/config.yaml` | 配置文件路径 |
| `--listen` | `0.0.0.0:8011` | 监听地址 |
| `--resources` | `resources` | YAML 资源目录（证候/问诊/方剂/调护/安全规则等） |
| `--tunnel-server` / `--tunnel-name` / `--tunnel-token` | 无 | 经 rrserver 隧道暴露本服务（见下节） |

**环境变量**：

| 变量 | 默认 | 说明 |
|---|---|---|
| `HARNESS_LISTEN` | `0.0.0.0:8011` | 监听地址 |
| `HARNESS_LLM_BASE_URL` | `http://localhost:11223/v1` | LLM 端点：直连 LM Studio，或 `http://llm_server:8000/v1`（网关）、`http://host.docker.internal:11223/v1`（容器内直连宿主机） |
| `HARNESS_LLM_API_KEY` | 空 | 上游 Key（LM Studio 开启 API Key 校验时必填） |
| `HARNESS_MODEL` | `google/gemma-4-12b-qat` | 默认模型 |
| `HARNESS_RESOURCES_DIR` | `resources` | 资源目录 |
| `HARNESS_RAG_ENDPOINT` | 无 | 可选 RAG 检索端点 |
| `HARNESS_TUNNEL_SERVER` / `_NAME` / `_TOKEN` | 无 | 隧道（等价于 `--tunnel-*`） |

> 注意：变量名前缀是 **`HARNESS_`**（不是 `TCM_`）。

> **无 LLM 时的可用性**：harness 未提供 MockProvider，`/chat` 问诊推进需要真实 LLM
> （LM Studio 或 llm_server 网关）；`/health`、`/agents`、`/skills` 等只读端点不受影响。
> 确定性逻辑（证候推断、配伍禁忌、方剂检索）的离线验证见
> [`testing.md`](./testing.md) 的 `cargo test -p harness --test cases`。

### 3.4 修改 YAML 资源后生效
- 重启 harness，或调用 `POST /reload` 热加载资源（需开启 `hot_reload`）。
- 资源字段 **key 为英文 slug、值为中文并附中文注释**，供中医专业人员维护；
  改完建议跑 `cargo test -p harness --test cases` 确认未破坏既有病例。

---

## 4. llm_server 部署

`llm_server/` 是纯 LM Studio 网关（**不托管模型**）。详见 [`llm_server.md`](./llm_server.md)。

```bash
cd llm_server
pip install -r requirements.txt
python -m app.main                       # 本地 :8000
docker compose up --build                # 容器 :8000 -> 宿主机 22010
```
核心配置：`LMSTUDIO_BASE_URL`（Docker 内用 `host.docker.internal:11223/v1`）、
`DEFAULT_MODEL=google/gemma-4-12b-qat`、`LLM_HOST/LLM_PORT`、`ENABLE_MCP`、`AGENT_MAX_ROUNDS`。

---

## 5. rrserver 部署（家庭算力上云，可选）

`server/rrserver/` 为 Rust 反向隧道：云端 **server**（中继，`8088`） + 家庭端 **client**（连 server 的 WS，转发本地 LLM 到 `/t/<name>/*`）。

### 5.1 构建（需在 Linux/WSL 或本地 Rust 环境）
```bash
cd server
cargo build --release -p rrserver     # 产物 server/target/release/rrserver（Windows 为 .exe）
```
> 镜像**在 Docker 内编译**（多阶段构建）：`rust:1.98-bookworm` 负责编译，
> `ubuntu:24.04` 作为运行镜像。两者 glibc 向前兼容（2.36 编译 → 2.39 运行），
> 不会出现 "GLIBC_2.39 not found"。
> 宿主机构建机无需安装 Rust 工具链，也**不使用**本地 `cargo build` 产物。
> 依赖层由 Docker 缓存复用（先复制清单 + 占位源码预编译依赖，再复制真实源码）。

### 5.2 启动
```bash
# 手动启动 server：
rrserver server --listen 0.0.0.0:8080 --config config/rrserver.toml   # 容器内 8080，nginx 8088 对外

# 家庭端 client（另一台机器 / 另一进程）：
rrserver client --server https://rr.windblue.tech \
               --name home --token <TOKEN> \
               --local http://host.docker.internal:8900           # 家庭端本地 LLM 地址
```
配置要点（`server/rrserver/config/rrserver.toml`）：`external_ws_base`（对外 WS 基址）、
`[[tunnels]]`（`name`/`token`）；client 经 `/api/register` 用 token 换取 `ws_url` 并建立隧道。

> **配置文件**：镜像内已内置一份默认配置（由 `config/rrserver.toml.example` 生成，且
> **示例 token 被清空**），因此 `docker run tcm-rrserver:local` 不带挂载也能启动。
> 生产必须由 compose 挂载真实配置覆盖（`deploy/docker-compose.yml` 已配）：
> `../server/rrserver/config/rrserver.toml:/etc/rrserver.toml:ro`，
> 并把 `token` 与 `external_ws_base` 改成实际值。
>
> ⚠️ `server/rrserver/start_rrserver.ps1` 中的路径仍指向已迁移前的 `tcm_work/rrserver`，
> 当前不可用；请按上面的手动命令启动。修复见 [`tasks.md`](./tasks.md) T1.9。

### 5.2.1 把 harness 经隧道暴露（无需额外家庭端进程）

harness 内置了隧道客户端，启动时提供 `--tunnel-server` + `--tunnel-name` 即可：

```bash
cd server/harness
../target/release/harness --listen 0.0.0.0:8011 \
  --tunnel-server ws://<云端 rrserver 地址> --tunnel-name tcm --tunnel-token <TOKEN>
# 等价环境变量：HARNESS_TUNNEL_SERVER / HARNESS_TUNNEL_NAME / HARNESS_TUNNEL_TOKEN
# 之后公网访问 https://<域名>/rr/t/tcm/* 即到达 harness
```
> 直连 rrserver server（不经过 nginx 反代）时，`external_ws_base` 应设为
> `ws://<host>:<port>`（**不含 `/rr` 前缀**）——`/rr` 前缀由 nginx 添加，
> 否则 WS 握手会 404。

### 5.3 防火墙与证书
- 云端放通 `8088`（server 入口）、`443`（HTTPS/WSS，由 `deploy/` 的统一 nginx 前置 TLS）。
- server 证书：`deploy/certs/fullchain.pem` + `privkey.pem`（自签名，WSL2 `openssl` 生成；
  nginx 配置见 `deploy/nginx/rrserver.conf`）；缺失会导致 nginx 反复重启。
- client 出站需可达 server 的 WSS 地址与家庭 LLM 端口。

---

## 6. 全链路验证

- **全链路 E2E**（harness→rrserver→llm_server）：见 [`e2e.md`](./e2e.md)，
  一键脚本 `tcm_work/e2e_tests/run_full_chain_e2e.ps1`。
- **后端测试**：见 [`testing.md`](./testing.md)，`cd server && cargo test -p harness`
  （含 `cases.jsonl` 案例回归）。
- **健康探针**：
  - harness：`GET /health`（经 nginx 为 `/api/health`）；另 `GET /agents`、`GET /skills`
  - llm_server：`GET /healthz`（无上游→`degraded`）
  - rrserver server：`GET /healthz`（`ok`）
  - 隧道通断：`GET /t/<name>/v1/models`（应转发到家庭端本地 LLM）；
    harness 隧道可用 `GET /t/tcm/skills` 验证
