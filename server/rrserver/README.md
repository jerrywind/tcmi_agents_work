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
                                    本项目 backend (TCM_LLM_BASE_URL)
```

## 工作原理

1. **注册**：家庭端 `client` 启动后，用 `name + token` 向云端 `POST /api/register`，
   云端校验通过后返回 `ws_url`。
2. **建连**：家庭端与云端 `GET /ws/<name>?token=...` 建立**持久 WebSocket**（出海连接，绕过 NAT）。
   云端每 25s 发一次 `ping` 心跳保活。
3. **转发**：外部对 `https://<域名>/rr/t/<name>/...` 的请求，经 nginx 反代到 rrserver；
   rrserver 通过对应隧道把请求（方法/路径/头/body）发给家庭端，家庭端转发到本地 `llm_server`，
   再把响应原路回传，rrserver 返回给外部调用方。整个 body 走 base64，支持任意二进制（含图片）。

## 目录结构

```
rrserver/
├── Cargo.toml
├── Dockerfile
├── docker-compose.yml
├── config/rrserver.toml.example
├── src/
│   ├── main.rs        # CLI：server / client / llm-server 子命令 + TOML 配置
│   ├── protocol.rs    # WS JSON 协议 + 头过滤/base64
│   ├── state.rs       # 隧道注册表（通道 + 请求等待映射）
│   ├── server.rs      # 云端中继：注册端点 + WS 隧道 + 反代转发 + 技能闸门
│   ├── client.rs      # 家庭端：注册 + 维持 WS + 转发本地 llm
│   ├── skill.rs       # 可选技能闸门：冷却/资源/状态前置校验引擎
│   └── llmsrv.rs      # 模型部署包装：启动/接入本地模型并注册到 rrserver
├── tests/integration.rs   # 27 个端到端集成测试（真实 TCP + WS 编排）
├── examples/e2e.rs        # 全链路实证示例（含真·流式分片透传）
├── scripts/gen-certs.sh   # 本地联调自签名证书生成
└── README.md
```

## 测试与质量

```bash
cargo test            # lib 70 + main 3 + integration 27 = 100 个测试
cargo run --example e2e   # 多组件编排实证：云端 ↔ 隧道 ↔ 本地（含流式）
```

集成测试覆盖：流式分片重组、非 2xx 状态透传、并发请求隔离、CORS（实际请求 +
OPTIONS 预检）、技能闸门（429/409/402、多技能冷却隔离）、隧道断线重连、
同名新连接替换旧隧道（守护 `unregister_if_same` 竞态修复）。

## 构建

需要 Rust 1.82+ 工具链（依赖均用 rustls，无需系统 OpenSSL）。

```bash
cd rrserver
cargo build --release --locked   # 已提交 Cargo.lock，--locked 保证可复现构建
# 产物：target/release/rrserver
```

> 2 核 2G 的服务器本地 `cargo build --release` 可能内存不足。推荐在开发机构建后拷贝二进制，
> 或用 Docker 在更高配置机器 `docker build` 后拉取镜像。

## Docker 封装

项目附带多阶段 `Dockerfile`（`rust:1.82-slim` 构建 + `debian:bookworm-slim` 运行）。
因为本项目 TLS 全部走 **rustls**，**运行时不需要系统 OpenSSL**，镜像内仅安装 `ca-certificates`
即可对外建立 TLS 连接；镜像以**非 root 用户**（`uid 10001`）运行。

构建镜像：

```bash
cd rrserver
docker build -t rrserver:local .
```

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
本项目 backend 通过环境变量指向云端隧道：

```bash
export TCM_LLM_BASE_URL=https://<你的域名>/rr/t/home/v1
export TCM_LLM_API_KEY=sk-xxx
```

这样 backend 的所有 LLM 调用会被隧道转发到家庭内的 `llm_server`，
在家庭 GPU 机器上跑大模型、在云端廉价服务器上做中转，既省钱又能用上本地算力。

## 配置项

| 位置 | 项 | 说明 |
|---|---|---|
| server `--listen` | 监听地址 | 默认 `127.0.0.1:8080`（仅内网） |
| config `external_ws_base` | 外部 WS 基址 | 家庭端拼接 `ws_url` 用，如 `wss://rr.example.com/rr` |
| config `[[tunnels]]` | name/token | 允许的隧道鉴权对，可多个 |
| client `--server` | 云端基址 | 如 `https://rr.example.com/rr` |
| client `--local` | 本地 llm 基址 | 如 `http://127.0.0.1:8080` |

## 安全说明

- 隧道采用 `name + token` 双向校验（注册与 WS 接入都验 token），请勿使用弱 token。
- rrserver 仅监听内网，由 `deploy/` 的统一 nginx 做 TLS 与对外暴露；不要在公网直接暴露 rrserver 端口。
- `/rr/t/<name>/` 路径中的 `<name>` 即为隧道标识，泄露等同于暴露该隧道的入口，建议 name 也具随机性。
- 本隧道不加密家庭端与云端之间的数据内容（依赖 nginx/wss 的 TLS）；如需端到端加密可后续在 client/server 增加应用层加密。
