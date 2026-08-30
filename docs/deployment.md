# 部署文档（Deployment）

四组件的部署：**前端 / harness / llm_server / rrserver**，以及端口、配置、网络与上线检查清单。

> **后端完全依赖 Docker**：harness 与 rrserver 的镜像都是多阶段构建（容器内编译），
> 不使用宿主机 `cargo build` 产物，构建机无需 Rust 工具链。
> 模型与降级事实见 [`llm_server.md`](./llm_server.md)：`llm_server` 是纯 LM Studio 网关
> （**不托管模型**），模型 `google/gemma-4-12b-qat`，LM Studio 默认 `:11223`；
> harness **无 MockProvider**，无 LLM 时只读端点可用、`/chat` 会失败。

---

## 1. 端口与地址（单一事实源）

| 组件 | 端口 | 说明 |
|---|---|---|
| 前端 dev | 10086 | `npm run dev:h5` |
| 前端生产（nginx） | 8080 | 静态产物托管 |
| harness | 8011 | 容器内监听；nginx 以 `/api` 前缀对外并剥离后转发 |
| llm_server | 8000 | `/healthz`、`/v1/*`（可选网关） |
| LM Studio（宿主机） | 11223 | `http://localhost:11223/v1`；容器内用 `host.docker.internal` |
| rrserver server | 容器内 8080（nginx 对外 8088） | `/healthz`、`/api/register`、WS `/ws/<name>`、隧道 `/t/<name>/*` |
| rrserver client | 9000 | 家庭端隧道客户端 |
| RAG（可选） | 8080（`RAG_PORT`） | `llm_server/rag`，见 `rag.md` |

> rrserver 对外入口统一为 `8088`（`deploy/nginx/rrserver.conf`），不是早期的 8080 直连。
> RAG 与前端生产都用 8080，但分属不同容器/网络，不冲突。

---

## 2. 前端部署

```bash
cd frontend
npm install && npm run dev:h5        # 开发：http://localhost:10086
npm run build:h5                     # 生产：产物在 dist/
```

- `apiBase` 由 `config/dev.ts`（或 `process.env.VITE_API_BASE`）指定，指向 harness。
- 生产静态托管与反代统一由 `deploy/` 的 nginx 完成：
  - `nginx/frontend.conf`：托管 `dist/`（SPA 回退）+ 反代 `/api` 到 harness（剥离前缀）。
    harness 不落盘图片，无 `/uploads` 目录。
  - `nginx/rrserver.conf`：TLS 终止 + 反代 `/rr` 到 rrserver。
- 启动：`docker compose -f deploy/docker-compose.yml up -d --build`
- 微信小程序：开发者工具导入 `frontend/`，走小程序审核发布流程（资质要求见第 7 节）。

---

## 3. harness 部署（Rust 后端）

### 3.1 出镜像并运行

```powershell
cd server                                    # 构建上下文必须是 workspace 根
docker build -f harness/Dockerfile -t tcm-harness:local .
docker run -d --name tcm-harness-8011 -p 8011:8011 `
  -e HARNESS_LLM_BASE_URL=http://host.docker.internal:11223/v1 `
  -e HARNESS_LLM_API_KEY=<LM Studio 令牌> `
  tcm-harness:local
# 验证：http://127.0.0.1:8011/health 返回 ok
```

一键出镜像（等价）：`pwsh scripts\build-release.ps1`。

- **为什么构建上下文必须是 `server/`**：harness 与 rrserver 同属一个 Cargo workspace，
  且 harness 依赖 rrserver 的 lib，两者源码都要进上下文。
- 镜像内默认 `--listen 0.0.0.0:8011 --resources /data/resources`；
  改 YAML 无需重建镜像——compose 已挂载 `resources:/data/resources:ro`，
  再 `POST /reload`（需 `hot_reload: true`）或重启即可。
- 依赖层由 Docker 缓存复用（先复制清单 + 占位源码预编译依赖，再复制真实源码）。

### 3.2 配置

优先级（低→高）：`resources/config.yaml` → 环境变量 `HARNESS_*` → 命令行参数。

**命令行参数**（仅 4 类，其余走配置/环境变量）：

| 参数 | 默认 | 说明 |
|---|---|---|
| `--config` | `resources/config.yaml` | 配置文件路径 |
| `--listen` | `0.0.0.0:8011` | 监听地址 |
| `--resources` | `resources` | YAML 资源目录 |
| `--tunnel-server` / `--tunnel-name` / `--tunnel-token` | 无 | 经 rrserver 隧道暴露本服务（见 5.3） |

**环境变量**：

| 变量 | 默认 | 说明 |
|---|---|---|
| `HARNESS_LISTEN` | `0.0.0.0:8011` | 监听地址 |
| `HARNESS_LLM_BASE_URL` | `http://localhost:11223/v1` | LLM 端点：直连 LM Studio、经网关 `http://llm_server:8000/v1`、容器内直连宿主机 `host.docker.internal` |
| `HARNESS_LLM_API_KEY` | 空 | 上游 Key（LM Studio 开启校验时必填） |
| `HARNESS_MODEL` | `google/gemma-4-12b-qat` | 模型 |
| `HARNESS_RESOURCES_DIR` | `resources` | 资源目录 |
| `HARNESS_LLM_TIMEOUT_SECS` | `120` | 单次 LLM 调用超时 |
| `HARNESS_MAX_TOOL_ROUNDS` | `3` | 工具调用最大轮数（1 = 退化为一轮） |
| `HARNESS_LLM_MAX_RETRIES` | `2` | LLM 重试次数（仅超时/连接失败/5xx/429） |
| `HARNESS_LLM_RETRY_BACKOFF_MS` | `500` | 重试退避基数 |
| `HARNESS_RAG_ENDPOINT` | 无 | 可选 RAG 检索端点，见 `rag.md` |
| `HARNESS_MCP_CLIENTS` | 无 | 外部 MCP server：`name=kb,url=http://host/mcp;name=...,url=...`，见 `mcp.md` |
| `HARNESS_STORE_DIR` | 无 | **报告持久化目录；不设置则不落盘**（harness 保持无状态，见 `usage.md` 2.8） |
| `HARNESS_STORE_REDACT` | `true` | 落盘前脱敏（手机号/身份证/邮箱/长数字串） |
| `HARNESS_STORE_LIST_LIMIT` | `20` | `GET /reports` 返回条数上限 |
| `HARNESS_TUNNEL_SERVER` / `_NAME` / `_TOKEN` | 无 | 隧道（等价 `--tunnel-*`） |

> 变量名前缀是 **`HARNESS_`**（不是 `TCM_`）。

### 3.3 改完 YAML 如何生效

重启，或 `POST /reload`（需 `hot_reload: true`）。改完建议跑一遍确定性回归
确认未破坏既有病例（Docker 内执行，见 [`testing.md`](./testing.md)）。

---

## 4. llm_server 部署（可选网关）

`llm_server/` 是纯 LM Studio 网关（**不托管模型**），详见 [`llm_server.md`](./llm_server.md)。

```bash
cd llm_server
pip install -r requirements.txt
python -m app.main                       # 本地 :8000

# 或容器：把 .env.example 复制为 .env 后
docker compose up --build                # 容器 :8000
```

核心配置：`LMSTUDIO_BASE_URL`（Docker 内用 `host.docker.internal:11223/v1`）、
`LMSTUDIO_API_KEY`、`DEFAULT_MODEL`、`LLM_HOST/LLM_PORT`、`ENABLE_MCP`、`AGENT_MAX_ROUNDS`。

> ⚠️ **不要把真实 Key 写进 `.env.example`**——该文件不被 `.gitignore` 忽略，会被提交。
> 真实 Key 放 `.env`（已被忽略，compose 直接读取）。

---

## 5. rrserver 部署（家庭算力上云，可选）

Rust 反向隧道：云端 **server**（中继） + 家庭端 **client**（建 WS 隧道，把本地 LLM 暴露到 `/t/<name>/*`）。

### 5.1 构建

```powershell
cd server
docker build -f rrserver/Dockerfile -t tcm-rrserver:local .
```

镜像内编译（`rust:1.98-bookworm` 编译 → `ubuntu:24.04` 运行，glibc 向前兼容），
宿主机构建机无需 Rust 工具链。

### 5.2 启动

```bash
# 云端 server（容器内监听 8080，nginx 8088 对外）
docker run -d --name rrserver -p 8088:8080 `
  -v $PWD/rrserver/config/rrserver.toml:/etc/rrserver.toml:ro `
  tcm-rrserver:local server --listen 0.0.0.0:8080 --config /etc/rrserver.toml

# 家庭端 client（另一台机器）
docker run -d --name rrclient tcm-rrserver:local client `
  --server https://rr.windblue.tech --name home --token <TOKEN> `
  --local http://host.docker.internal:8900
```

配置要点（`server/rrserver/config/rrserver.toml`）：`external_ws_base`（对外 WS 基址）、
`[[tunnels]]`（`name`/`token`）；client 经 `/api/register` 用 token 换 `ws_url` 并建立隧道。

> 镜像内已内置一份默认配置（示例 token 已清空），不带挂载也能启动；
> **生产必须由 compose 挂载真实配置覆盖**，并把 token 与 `external_ws_base` 改成实际值。
> 本地一键启动也可用 `server/rrserver/start_rrserver.ps1`（路径已对齐当前仓库结构）。

### 5.3 把 harness 经隧道暴露（无需额外家庭端进程）

harness 内置隧道客户端，启动时提供 server + name 即可：

```powershell
docker run -d --name tcm-harness-8011 -p 8011:8011 `
  -e HARNESS_TUNNEL_SERVER=ws://<云端 rrserver> -e HARNESS_TUNNEL_NAME=tcm `
  -e HARNESS_TUNNEL_TOKEN=<TOKEN> tcm-harness:local
# 之后公网访问 https://<域名>/rr/t/tcm/* 即到达 harness
```

> 直连 rrserver server（不经 nginx 反代）时，`external_ws_base` 应设为 `ws://<host>:<port>`
> （**不含 `/rr` 前缀**）——`/rr` 由 nginx 添加，否则 WS 握手 404。

### 5.4 防火墙与证书

- 云端放通 `8088`（server 入口）、`443`（HTTPS/WSS，由 `deploy/` 的 nginx 前置 TLS）。
- 证书：`deploy/certs/fullchain.pem` + `privkey.pem`（当前为自签名，仅内网可用；
  生产需换成可信证书）。缺失会导致 nginx 反复重启。
- client 出站需可达 server 的 WSS 地址与家庭 LLM 端口。

---

## 6. 全链路验证

- **健康探针**：harness `GET /health`（经 nginx 为 `/api/health`）；llm_server `GET /healthz`
  （无上游→`degraded`）；rrserver `GET /healthz`（`ok`）；
  隧道通断 `GET /t/<name>/v1/models`，harness 隧道可用 `GET /t/tcm/skills` 验证。
- **全链路 E2E**（用 stub，不需真实 LLM）：`e2e_tests/run_full_chain_e2e.ps1`，见 [`e2e.md`](./e2e.md)。
- **人工验收**（需真实 LLM）：`e2e_tests/run_manual_e2e.ps1`，产出见 [`samples/`](./samples/README.md)。
- **后端测试**（Docker 内）：见 [`testing.md`](./testing.md)。

---

## 7. 上线检查清单（T5.2 / T5.3）

T5.1（报告持久化）与 T5.4（合规）已落地；T5.2、T5.3 **依赖外部云资源**，
本机无法完成，资源到位后逐项勾选。

### 7.1 T5.2 对象存储与媒体上云

现状：**harness 不保存图片**——舌象/面相以 base64 或 URL 随请求传入，用完即弃，
没有上传目录，也不依赖对象存储。接入前先明确要解决「前端静态资源」与「用户图片」两件事：

| 项 | 做什么 | 状态 |
|---|---|---|
| 1 | 建桶（私有），开启服务端加密与版本控制 | ☐ 待资源 |
| 2 | 前端 `dist/` 上传（`aws s3 sync dist/ s3://<bucket>/h5/ --delete`） | ☐ 待资源 |
| 3 | 桶策略：仅 CDN / nginx 回源可读，禁止公共读 | ☐ 待资源 |
| 4 | 若做「报告附图」：harness 需新增上传端点，URL 随报告归档 | ☐ 未开工 |
| 5 | 生命周期规则：报告与图片保留期（建议 ≤ 180 天） | ☐ 待资源 |
| 6 | 密钥走 IAM 角色 / 环境变量，**不入库、不进镜像** | ☐ 待资源 |

> 一旦 harness 开始保存用户图片，就多了一处需要保护的个人信息，
> 需同步满足 T5.4 的脱敏与删除要求——**合规复核通过前不要开通**。

### 7.2 T5.3 CDN 与小程序上架

| 项 | 做什么 | 状态 |
|---|---|---|
| 1 | 域名备案 + 可信 HTTPS 证书（当前自签名仅内网可用） | ☐ 待资源 |
| 2 | CDN 加速域名指向源站，缓存规则（HTML 不缓存，hash 资源长缓存） | ☐ 待资源 |
| 3 | 回源鉴权（Referer / 签名 URL）防刷量 | ☐ 待资源 |
| 4 | 小程序：`npm run build:weapp` → 开发者工具上传体验版 | ☐ 待资源 |
| 5 | 提审材料：医疗类目需**医疗机构执业许可证或互联网医疗备案**，<br>个人主体通常无法通过 | ⚠️ 需确认主体资质 |
| 6 | 自查：免责声明常驻不可关闭、无「诊断/治疗」表述、红旗引导就医 | 已由 T5.4 服务端保证 |
| 7 | 服务端域名加入小程序 request 合法域名白名单 | ☐ 待资源 |
| 8 | 互联网医疗保健信息服务审核通过 | ☐ 待资源 |

> 第 5 项是**最大的非技术风险**：小程序类目审核看资质不看代码。
> 若主体不具备医疗资质，替代路径是只做 H5 + 公众号，或与持证机构合作。

### 7.3 T5.1 / T5.4 已落地部分

| 项 | 落地位置 |
|---|---|
| 报告落盘与回查 | `src/store.rs` + `GET /reports`、`GET /reports/:id` |
| 落盘脱敏 | `store.rs::redact_text`，`HARNESS_STORE_REDACT` 可关 |
| 路径穿越防护 | 报告 id 仅允许 `A-Za-z0-9_-`，最长 128 |
| 免责声明随结果下发 | `orchestrator::DISCLAIMER` → `/chat` 的 `disclaimer` 字段 |
| 安全门不可被配置移除 | `orchestrator::resolve_order`（缺 safety 时强制插入并告警） |
| 前端存证 / 回查 | 报告页「存证与回查」+ `pages/reports` 存证记录页 |
