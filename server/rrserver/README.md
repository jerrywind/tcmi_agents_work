# rrserver — 家庭 LLM 反向隧道中继

在家庭网络内部署的 LLM 服务（无公网 IP / 无端口映射）通过一个**主动建立的长连接**，
把自身「注册」到云服务器上的 `rrserver`，从而让外部（本项目后端、前端、任意 OpenAI 兼容客户端）
经由云端安全访问家庭内的 LLM。本质上是 frp/ngrok 式的反向隧道，但为「家庭 LLM 即服务」场景定制，
且用 Rust 编写，资源占用极低，适合 **2 核 2G** 的轻量云服务器。

```
       家庭网络 (无公网IP)                云服务器 (2核2G)
  ┌─────────────────────┐           ┌────────────────────────────┐
  │  llm_server         │           │  nginx (TLS终端/反代 /rr/)  │
  │  127.0.0.1:8080     │           │       │                     │
  │       ▲             │  主动出站  │       ▼                     │
  │  rrserver client    │ ─WS隧道──▶ │  rrserver (127.0.0.1:8080) │
  │  (本二进制 client)  │  (控制+数据)│  注册 /api/register        │
  └─────────────────────┘           │  隧道 /t/<name>/...         │
                                     └────────────────────────────┘
                                            │ 外部请求
                                            ▼
                                    本项目 harness (HARNESS_LLM_BASE_URL)
```

## 工作原理

1. **注册**：家庭端 `client`（或 `llm_server` 自身）启动后，用 `name + token` 向云端
   `POST /api/register`，云端校验通过后会为该次注册签发一个**独立 hash code**，
   并返回 `ws_url`、接入形态（`transport`）与心跳周期。
2. **建连**：家庭端与云端 `GET /ws/<name>?token=...` 建立**持久 WebSocket**（出海连接，绕过 NAT）。
   云端每 25s 发一次 `ping` 心跳保活。
3. **转发**：外部对 `https://<域名>/rr/t/<name>/...` 的请求，经 nginx 反代到 rrserver；
   rrserver 通过对应隧道把请求（方法/路径/头/body）发给家庭端，家庭端转发到本地 `llm_server`，
   再把响应原路回传，rrserver 返回给外部调用方。整个 body 走 base64，支持任意二进制（含图片）。
4. **心跳与探活**：见下节「注册 · 心跳 · 探活」。

## 注册 · 心跳 · 探活

服务注册后会拿到一个 hash code，随后由两端共同维护「服务是否还活着」：

| 环节 | 周期 / 阈值 | 说明 |
|---|---|---|
| 服务主动心跳 | 每 **30 分钟** | `POST /api/heartbeat {"name","hash"}`，证明服务仍活跃 |
| 转发首响等待 | **1 分钟** | 转发后 1 分钟没等到首个字节，云端**主动探活**，确认服务是否在运行 |
| 静默判定 | **40 分钟** | 云端每 60s 扫描一次，找出 40 分钟没心跳的注册 |
| 主动探活 | 等待 **1 分钟** | 经 WS 隧道下发 `heartbeat{probe_id}`，或（HTTP 直连形态）`GET {endpoint}/rr/heartbeat` |
| 探活失败处理 | 立即 | 记录 `WARN` 日志 → 注销该注册、关闭对应隧道通道 |

要点：

- **hash code 每次注册重新签发**：同名服务重连/重启会拿到新 hash，可用于识别「重新注册」。
  它同时可作为 WS 接入凭证（`?token=<hash>`），心跳/注销也以它为准。
- **探活成功即保留**：服务确实在运行但心跳缺失（例如心跳任务异常）时只记日志、不注销；
  并记下探活时间，避免下一轮重复探活刷爆日志。
- **注销后自动重连**：注册被回收后，家庭端下一次心跳会收到 404，随即断开并重连、
  重新注册拿新 hash；`llm_server`（Python）侧同样会自动重新注册。
- **两种接入形态**：
  - `ws`（默认）：服务在 NAT 后，走反向隧道，探活经 WS 下发；
  - `http`：注册时带 `endpoint`（云端可直达的基址），探活走 `GET {endpoint}/rr/heartbeat`。

相关端点：

| 端点 | 说明 |
|---|---|
| `POST /api/register` | 注册并换取 hash code；可带 `endpoint` / `transport`。周期**由云端决定**并随响应下发 |
| `POST /api/heartbeat` | 心跳上报（`{name, hash}`，name 必须与注册名一致）；注册不存在或 name 不符返回 404 |
| `POST /api/unregister` | 主动注销：关闭注册维护并断开隧道（同样校验 name） |
| `GET /api/services` | 注册总览（hash、形态、已过时长、是否 stale） |

约定：

- **周期 / 配置类字段统一用毫秒**（`*_millis`）：`heartbeat_interval_millis`、
  `heartbeat_timeout_millis`、`probe_timeout_millis`。秒级字段在亚秒场景下会退化成 0，
  配置文件里仍是秒（`[health] heartbeat_interval_secs`）。
- **已过去时长的字段用秒**（`heartbeat_age_secs`、`silence_secs`、`registered_secs_ago`），
  便于人工阅读。
- **服务必须先注册**才能被转发：注册是拿到隧道地址的唯一途径，注册记录里带着探活所需的
  形态与地址。隧道在而注册没了时，转发会返回 **504 `service not registered`**，
  服务重新注册后即恢复。

全部时长可在配置的 `[health]` 表中覆盖（见 `config/rrserver.toml`）。

## 目录结构

```
rrserver/
├── Cargo.toml
├── Dockerfile
├── docker-compose.yml
├── config/rrserver.toml.example
├── src/
│   ├── main.rs        # CLI：server / client / llm-server 子命令 + TOML 配置
│   ├── protocol.rs    # WS JSON 协议 + 头过滤/base64 + 心跳探活消息
│   ├── state.rs       # 隧道注册表（通道 + 请求等待映射 + 探活等待映射）
│   ├── registry.rs    # 服务注册中心：hash code 签发、心跳记录、静默扫描
│   ├── server.rs      # 云端中继：注册/心跳/注销端点 + WS 隧道 + 转发探活 + 回收任务 + 技能闸门
│   ├── client.rs      # 家庭端：注册 + 维持 WS + 心跳上报 + 转发本地 llm
│   ├── skill.rs       # 可选技能闸门：冷却/资源/状态前置校验引擎
│   └── llmsrv.rs      # 模型部署包装：启动/接入本地模型并注册到 rrserver
├── tests/integration.rs   # 36 个端到端集成测试（真实 TCP + WS 编排）
├── examples/e2e.rs        # 全链路实证示例（含真·流式分片透传）
├── scripts/gen-certs.sh   # 本地联调自签名证书生成
└── README.md
```

## 测试与质量

```powershell
# 后端一律在 Docker 内跑（不使用宿主机 cargo 产物）
cd server
docker run --rm -v "${PWD}:/build" -w /build rust:1.98-bookworm `
  cargo test -p rrserver          # lib 107 + main 4 + integration 36 = 147 个测试
```

集成测试覆盖：流式分片重组、非 2xx 状态透传、并发请求隔离、CORS（实际请求 +
OPTIONS 预检）、技能闸门（429/409/402、多技能冷却隔离）、隧道断线重连、
同名新连接替换旧隧道（守护 `unregister_if_same` 竞态修复）、
注册签发 hash code / 心跳保活 / 探活失败后注销注册（回收任务）、
转发首响超时后主动探活并继续等待（慢推理不被 1 分钟窗口掐断）、
真实 client 心跳链路、注销后自动重连并重新注册（换新 hash）、
**隧道在但注册没了 → 504 并提示重新注册**（重新注册后即恢复）、
心跳 name 与注册名不一致按未知注册处理。

全链路实证示例（真实 TCP + WS + 流式）：`docker run ... cargo run -p rrserver --example e2e`。

## 构建

**完全依赖 Docker**：多阶段 `Dockerfile`（`rust:1.98-bookworm` 编译 → `ubuntu:24.04` 运行，
glibc 向前兼容），构建机无需 Rust 工具链，也**不使用**本地 `cargo build` 产物。
依赖层由 Docker 缓存复用（先复制清单 + 占位源码预编译依赖，再复制真实源码）。

```bash
cd server                                   # 构建上下文必须是 workspace 根
docker build -f rrserver/Dockerfile -t rrserver:local .
```

> 为什么上下文是 `server/`：rrserver 与 harness 同属一个 Cargo workspace，
> 依赖清单与源码都要进上下文。

TLS 全部走 **rustls**，运行时不需要系统 OpenSSL，镜像内仅装 `ca-certificates`，
并以**非 root 用户**（`uid 10001`）运行。

三个子命令都基于同一个镜像，靠 `command` 区分：

```bash
# 1) 云端中继（默认 command；也可 docker compose up）
docker run --rm -p 8080:8080 \
  -v "$PWD/config/rrserver.toml:/etc/rrserver.toml:ro" \
  rrserver:local server --listen 0.0.0.0:8080 --config /etc/rrserver.toml

# 2) 家庭端隧道客户端（把本地 llm 注册到云端；--local 指向宿主上的模型服务）
docker run --rm \
  -e RR_DOMAIN=https://<你的域名>/rr -e HOME_TOKEN=<token> \
  --add-host host.docker.internal:host-gateway \
  rrserver:local client --server "$RR_DOMAIN" --name home \
  --token "$HOME_TOKEN" --local http://host.docker.internal:8080

# 3) 模型部署包装（deploy + 注册一键完成）
docker run --rm \
  -v "$PWD/config/llm_server.toml:/etc/llm_server.toml:ro" \
  rrserver:local llm-server --config /etc/llm_server.toml
```

`docker-compose.yml` 默认只启动 `rrserver`（云端中继）。
nginx 反代 / TLS 已统一抽到仓库根 `deploy/`（见 `deploy/nginx/rrserver.conf` 与
`deploy/docker-compose.yml` 的 `nginx` 服务），`rrserver` 自身**不含 nginx**。
家庭端 `client` 与 `llm-server` 放在 `optional` profile 下，**默认不启动**：

```bash
# 生产：统一由 deploy 编排（nginx + rrserver + backend）
docker compose -f ../deploy/docker-compose.yml up -d --build

# 仅本地启动 rrserver（开发/调试，需自行解决对外暴露与 TLS）
docker compose up -d --build

# 额外拉起家庭端 client
docker compose --profile optional up -d client

# 额外拉起模型部署包装 llm-server（需先准备 config/llm_server.toml）
docker compose --profile optional up -d llm-server
```

> 构建上下文中的 `target/`、`.git`、`tests/`、`examples/` 已被 `.dockerignore` 排除，
> 不会进入镜像，保证构建干净、镜像小巧。

### 本地联调（自签名证书）

```bash
# 1. 生成自签名证书到 deploy/certs/（仅用于本机测试，浏览器会报不安全属正常）
bash scripts/gen-certs.sh   # 生成后把 fullchain.pem / privkey.pem 放到 deploy/certs/

# 2. 由顶层 deploy 编排启动 nginx + 云端中继
docker compose -f ../deploy/docker-compose.yml up -d --build

# 3. 健康检查（自签名证书需 -k 跳过校验）
curl -k https://localhost/rr/healthz   # 期望输出 ok
```

生产部署请把 `deploy/certs/` 换成真实证书（如 certbot 申请的 `fullchain.pem`/`privkey.pem`），
并把 `config/rrserver.toml` 里的 `external_ws_base` 与隧道 `token` 改成真实值。

### 关于流式（LLM 增量输出）

全链路为**真·流式**（边生成边转发）：

- 家庭端 `client` 用 `Response::chunk()` 从本地 LLM 逐段读取响应体，**每读到一段立即回传一片**
  （无需 reqwest `stream` 特性，离线亦可编译）；
- 云端 `proxy_handler` 把回传的 `ResponseChunk` 分片实时重组成流式/分块 HTTP 响应。

外部以 OpenAI 兼容方式调用 `/t/<name>/v1/chat` 时可获得逐 token / SSE 输出，
首字延迟不受响应总长度影响。`examples/e2e.rs` 中有分片间隔产出的端到端流式实证。

## 部署（云端）

1. 准备 TLS 证书到 `deploy/certs/{fullchain.pem,privkey.pem}`（可用 certbot，nginx 配置见 `deploy/nginx/rrserver.conf`）。
2. 复制并填写配置：
   ```bash
   cp config/rrserver.toml.example config/rrserver.toml
   # 修改 external_ws_base 为你的域名，token 改为强随机串
   ```
3. 启动（统一由仓库根 deploy 编排，nginx 反代 + rrserver 一起拉起）：
   ```bash
   docker compose -f ../deploy/docker-compose.yml up -d --build
   ```
   检查：`curl https://<你的域名>/rr/healthz` 应返回 `ok`。
   > rrserver 容器内在 `0.0.0.0:8080` 监听，仅供同网络下的 nginx 访问（不暴露宿主机端口）；
   > 配置通过 `./config/rrserver.toml` 挂载到 `/etc/rrserver.toml`。
   > nginx 配置位于 `deploy/nginx/rrserver.conf`，证书位于 `deploy/certs/`。

## 运行家庭端（家庭网络内，与 llm_server 同机/同网）

```bash
rrserver client \
  --server https://<你的域名>/rr \
  --name home \
  --token <与云端一致的 token> \
  --local http://127.0.0.1:8080
```

客户端会持续运行并自动重连。家庭端只需能访问外网，无需任何端口转发。

## 模型部署包装（`llm-server`）

`rrserver llm-server` 把「部署本地模型服务」与「注册到 rrserver 隧道」合并为一个可运行包装器，
免去你手动先启动模型服务、再单独跑 `client` 的两步操作，并对后端进程做生命周期监管。

它支持两种后端形态（在 `[backend]` 中用 `mode` 选择）：

- **`static`**：接入一个已经运行的本地模型服务（如手动启动的 vLLM / Ollama），包装器不启动新进程，只负责注册隧道。
- **`command`**：由包装器 `spawn` 一个模型服务进程并持续监管；启动后轮询 `health_url` 探针直到就绪，
  就绪后才把隧道指向它。进程随包装器退出而被杀死（Ctrl-C 优雅关停）。

可选 `info_port` 会让包装器额外暴露 `http://0.0.0.0:<port>/healthz` 与 OpenAI 风格的
`/v1/models`，便于容器探活与模型发现。

```bash
# 1) 准备配置
cp config/llm_server.toml.example config/llm_server.toml
# 2) 按需修改：backend（启动方式 / 模型 / 探针）、rrclient（隧道凭据）、可选 models/info_port
# 3) 运行包装器（deploy + 注册同时完成）
rrserver llm-server --config config/llm_server.toml
```

之后外部即可经 `https://<域名>/rr/t/home/v1` 访问到家庭内由包装器拉起的模型。

## 与本项目的对接

家庭内运行本仓库的 `llm_server`（OpenAI 兼容，基址 `http://127.0.0.1:8080/v1`）。
本项目 harness 通过环境变量指向云端隧道：

```bash
export HARNESS_LLM_BASE_URL=https://<你的域名>/rr/t/home/v1
export HARNESS_LLM_API_KEY=sk-xxx
```

这样 harness 的所有 LLM 调用会被隧道转发到家庭内的 `llm_server`，
在家庭 GPU 机器上跑大模型、在云端廉价服务器上做中转，既省钱又能用上本地算力。

> 前缀是 **`HARNESS_`**；`TCM_LLM_*` 是已废弃的旧写法，harness 不会读取。

harness 也可不经独立 client 进程、直接内置隧道暴露自身：

```powershell
docker run -d --name tcm-harness-8011 -p 8011:8011 `
  -e HARNESS_TUNNEL_SERVER=wss://<域名>/rr -e HARNESS_TUNNEL_NAME=tcm `
  -e HARNESS_TUNNEL_TOKEN=<TOKEN> tcm-harness:local
# 完整变量表见 docs/deployment.md 3.2
```

## 配置项

| 位置 | 项 | 说明 |
|---|---|---|
| server `--listen` | 监听地址 | 默认 `127.0.0.1:8080`（仅内网） |
| config `external_ws_base` | 外部 WS 基址 | 家庭端拼接 `ws_url` 用，如 `wss://rr.example.com/rr` |
| config `[[tunnels]]` | name/token | 允许的隧道鉴权对，可多个 |
| client `--server` | 云端基址 | 如 `https://rr.example.com/rr` |
| client `--local` | 本地 llm 基址 | 如 `http://127.0.0.1:8080` |
| config `[health]` | 心跳 / 探活 / 转发超时 | `heartbeat_interval_secs`、`heartbeat_timeout_secs`、`probe_timeout_secs`、`first_response_timeout_secs`、`request_timeout_secs`、`reaper_interval_secs` |

## 安全说明

- 隧道采用 `name + token` 双向校验（注册与 WS 接入都验 token），请勿使用弱 token。
- rrserver 仅监听内网，由 `deploy/` 的统一 nginx 做 TLS 与对外暴露；不要在公网直接暴露 rrserver 端口。
- `/rr/t/<name>/` 路径中的 `<name>` 即为隧道标识，泄露等同于暴露该隧道的入口，建议 name 也具随机性。
- 本隧道不加密家庭端与云端之间的数据内容（依赖 nginx/wss 的 TLS）；如需端到端加密可后续在 client/server 增加应用层加密。
