# 部署文档（Deployment）

后端 FastAPI 服务 + 前端 Taro 多端产物。以下给出从环境到上线的可操作指引。

## 1. 环境要求

| 组件 | 版本 | 备注 |
|---|---|---|
| Python | 3.10+ | 后端运行环境 |
| Node.js | 18+ | 前端构建 |
| `@tarojs/cli` | 4.x | 全局或本地 devDependency |
| （可选）PostgreSQL / Redis | 14+ / 6+ | 生产替代内存存储 |
| （可选）Docker | 20+ | 容器化部署 |

## 2. 后端部署

### 2.1 环境变量

通过环境变量或 `backend/app/config.py` 配置（环境变量优先级更高）：

| 变量 | 说明 | 默认值 |
|---|---|---|
| `TCM_PORT` | 服务端口 | `8000` |
| `TCM_HOST` | 监听地址 | `0.0.0.0` |
| `TCM_LLM_API_KEY` | LLM API Key（未设置则全程 mock 降级） | 空 |
| `TCM_LLM_BASE_URL` | OpenAI 兼容文本端点 base_url（留空则由 `routing.llm.yaml` 经 `TCM_ROUTING_FILE` 注入） | 空 |
| `TCM_LLM_PROVIDER` | 供应商标识 | `openai` |
| `TCM_LLM_TEXT_MODEL` | 文本逻辑模型映射 | `qwen3.6-9B` |
| `TCM_LLM_VISION_MODEL` | 视觉逻辑模型映射 | `Qwen3-VL-8B` |
| `TCM_LLM_VISION_BASE_URL` | 独立视觉端点 base_url（Qwen3-VL 原生多模态，无需 mmproj） | 空 |
| `TCM_LLM_API` | LLM 调用协议：`responses`（LM Studio Responses API，默认）或 `chat`（传统 Chat Completions） | `responses` |
| `TCM_CORS_ORIGINS` | 允许的跨域来源（逗号分隔） | `*` |
| `TCM_ROUTING_FILE` | 路由配置文件路径（覆盖默认 `routing.yaml`） | `backend/app/routing.yaml` |
| `TCM_SKILLS_DIR` | SKILL 技能目录（自动发现与热装载） | `backend/app/skills` |
| `TCM_STORE` | 会话存储：`memory`（默认）\| `redis` | `memory` |
| `TCM_REDIS_URL` | Redis 连接串（仅 `TCM_STORE=redis` 时生效） | `redis://localhost:6379/0` |
| `TCM_MCP_SERVER_ENABLED` | 是否挂载 MCP 端点（`0/false/no` 关闭） | `true` |
| `TCM_MCP_MOUNT_PATH` | MCP Streamable HTTP 挂载路径 | `/mcp` |
| `TCM_MCP_CALL_TIMEOUT` | 远程 MCP 工具调用超时（秒） | `30` |
| `TCM_MCP_CLIENTS` | JSON 数组，覆盖启动时自动连接的外部 MCP Server | `[]` |

> 环境变量优先级高于 `routing.yaml`：设置 `TCM_LLM_BASE_URL` / `TCM_LLM_TEXT_MODEL` / `TCM_LLM_VISION_MODEL` 即可在容器/编排环境覆盖模型映射，无需改配置文件。

### 2.2 本地 / 开发

```bash
cd backend
pip install -r requirements.txt
python -m app.main            # 等价于 uvicorn app.main:app，host/port 读 TCM_HOST/TCM_PORT
# 或
uvicorn app.main:app --host 0.0.0.0 --port 8000
```

### 2.3 生产（Gunicorn 多进程）

```bash
pip install gunicorn
# 多 worker 时需启用 Redis 共享会话态（TCM_STORE=redis，并启动 Redis）
TCM_STORE=redis TCM_REDIS_URL=redis://127.0.0.1:6379/0 \
  gunicorn app.main:app -k uvicorn.workers.UvicornWorker \
  -w 4 --bind 0.0.0.0:8000 --timeout 120
```
> 说明：默认 `MemoryStore` 为进程内存态，**多 worker 下会话不共享**。启用 `TCM_STORE=redis`（接口不变，已内置 `RedisStore`）即可安全多 worker；或仅用单 worker。

### 2.4 Docker

仓库已提供 `backend/Dockerfile`（Python 3.11 + uvicorn，可通过 `TCM_HOST`/`TCM_PORT` 覆盖），直接构建运行：

```bash
docker build -t tcm-backend ./backend
docker run -d -p 8000:8000 \
  -e TCM_LLM_API_KEY=sk-xxx \
  -e TCM_LLM_BASE_URL=https://your-endpoint/v1 \
  -e TCM_LLM_TEXT_MODEL=your-text-model \
  -e TCM_LLM_VISION_MODEL=your-vision-model \
  -v tcm-uploads:/app/uploads \
  tcm-backend
```

## 2.5 一键编排（docker-compose）

各服务的 compose 已下沉到各自目录，互不依赖仓库根编排：

- **backend**：`backend/docker-compose.yml`（backend + 可选 `redis` / `postgres`，profile 控制）
- **frontend**：`frontend/docker-compose.yml`（仅前端，默认映射 8080:80）
- **llm_server**：`llm_server/docker-compose.yml`（llm_server 文本 + 可选 `vision` 视觉）

```bash
# 后端（在 backend/）：仅启动 backend
cd backend && docker compose up --build

# 启用可选 Redis（多 worker 共享会话）：需把 TCM_STORE 设为 redis
cd backend && TCM_STORE=redis docker compose --profile redis up --build

# 可选 PostgreSQL 占位（待将 MemoryStore 替换为持久化存储时启用）
cd backend && docker compose --profile pg up --build

# 前端（在 frontend/）
cd frontend && docker compose up --build
```

环境变量可从 `.env` 读取（参考仓库 `.env.example`）。

### 2.6 本地大模型服务（可选）

若希望 Sub-Agent 真正调用 LLM（而非默认 rule），先准备 `llm_server/`（基于
llama.cpp，默认 **文本模型 qwen3.6-9B + 视觉模型 Qwen3-VL**）：

```bash
# 1) 先在 llm_server/models 放好权重（文本 GGUF + 视觉 GGUF，见 llm_server/README.md）
# 2) 在 llm_server/ 启动模型服务（含内置 RAG 8080），vision profile 同时拉起视觉服务
cd llm_server && docker compose --profile vision up --build

# 3) 在 backend/ 用 llm 覆盖文件启动，backend 自动指向两个模型服务并启用 routing.llm.yaml
cd backend && docker compose -f docker-compose.yml -f docker-compose.llm.yml --profile llm up --build
```

该模式下 backend 自动设置：
`TCM_LLM_BASE_URL=http://llm_server:8000/v1`、`TCM_LLM_TEXT_MODEL=qwen3.6-9B`、
`TCM_LLM_VISION_MODEL=Qwen3-VL-8B`、`TCM_LLM_VISION_BASE_URL=http://llm_vision:8000/v1`、
`TCM_ROUTING_FILE=/app/routing.llm.yaml`（各能力切换为 `llm` / `llm_vision` 实现）。
需将 `backend/` 与 `llm_server/` 的容器加入同一 docker 网络，backend 才能解析
`llm_server` / `llm_vision` 服务名。

### 2.6.1 本地开发最简易：用 LM Studio（无需权重/Docker）

直接采用 LM Studio（或 Ollama 等任意 OpenAI 兼容服务）作为 LLM 后端，本地开发更快：
在 LM Studio 加载多模态模型（如 `google/gemma-4-12b-qat`，文本/视觉共用同一端点），
开启本地服务器（默认 `http://localhost:11223/v1`），然后设置：

```bash
$env:TCM_LLM_BASE_URL="http://localhost:11223/v1"
$env:TCM_LLM_API_KEY="<LM Studio → Developer → Server Settings 中的 API Key>"
$env:TCM_LLM_TEXT_MODEL="google/gemma-4-12b-qat"
$env:TCM_LLM_VISION_MODEL="google/gemma-4-12b-qat"   # 视觉与文本共用同一端点
$env:TCM_LLM_API="responses"                          # 使用 LM Studio Responses API
$env:TCM_ROUTING_FILE="app/routing.llm.yaml"
```

> `routing.llm.yaml` 默认 `api: responses`，即通过 LM Studio 的 **Responses API**（`/v1/responses`）
> 调用；如需传统 Chat Completions，设 `TCM_LLM_API=chat` 或把 `routing.llm.yaml` 的 `llm.api`
> 改为 `chat`。若 LM Studio 开启了 API Key 校验，需填入 Developer → Server Settings → API Key
> 中的值；关闭校验则任意非空值均可。
> 仓库已提供 `backend/start_backend.ps1` 一键以该配置启动后端。

> 此方式无需下载 GGUF 权重、无需 Docker，适合本地联调；生产自建仍可用 `llm_server` 方案。

> 不接入本地模型时，系统保持**离线 rule 实现**，无需任何模型权重。
> 详见 [`llm_server.md`](./llm_server.md) 与 [`sub_agents.md`](./sub_agents.md)。

## 3. 前端构建与发布

### 3.1 配置 API 地址

- 开发：`frontend/config/dev.ts` 中 `apiBase: '/api'`（由 Taro devServer 代理到后端 8000）。
- 生产：`frontend/config/prod.ts` 改 `apiBase` 为后端公网地址（如 `https://api.example.com/api`），并确保该域名已配置 CORS。

### 3.2 构建产物

```bash
cd frontend
npm install

# H5（静态站点，可托管到 Nginx / 对象存储 / Vercel）
npm run build:h5
# 产物在 dist/ ，部署到任意静态服务器

# 微信小程序（生成 dist/ 小程序代码，用微信开发者工具上传）
npm run build:weapp

# React Native（如已配置，见 Taro RN 文档）
# npm run build:rn
```
`package.json` 脚本：`dev:h5/dev:weapp/build:h5/build:weapp` 等。

### 3.3 容器化（H5 + Nginx）

仓库已提供 `frontend/Dockerfile`（多阶段：Node 18 构建 `npm run build:h5` → Nginx 托管 `dist/`），并通过 `frontend/nginx.conf` 将 `/api` 与 `/uploads` 反向代理到后端 `backend:8000`。在 compose 中已自动编排；独立运行：

```bash
docker build -t tcm-frontend ./frontend
docker run -d -p 8080:80 tcm-frontend
# 访问 http://localhost:8080 ，API 请求由 Nginx 转发到 backend 服务
```

> 前端 H5 通过相对路径 `/api` 访问后端（见 `src/services/api.ts`），因此必须由同源网关/反代转发，无需额外配置 CORS。

### 3.4 H5 + Nginx 反代示例（独立部署）

```nginx
server {
    listen 80;
    server_name tcm.example.com;

    # 前端静态资源
    location / {
        root /var/www/tcm-h5;
        try_files $uri $uri/ /index.html;
    }

    # 后端 API 反代
    location /api/ {
        proxy_pass http://127.0.0.1:8000/api/;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
}
```
> 单域名反代可避免前端跨域，亦省去 CORS 配置。

## 4. 生产注意事项

1. **持久化**：默认 `MemoryStore` 仅适合单机单进程演示。多 worker / 多实例场景可设 `TCM_STORE=redis`（已内置 `RedisStore`，接口不变）；长期归档可进一步把 `app/store.py` 替换为 PostgreSQL。
2. **文件/图片存储**：舌象、面相、患处照片建议走对象存储（OSS/S3）而非本地磁盘；`/api/consultations/{id}/images`（演示版）需替换为带签名 URL 的实现。
3. **HTTPS**：对外服务必须启用 TLS，避免健康数据明文传输。
4. **CORS**：若前端与后端跨域，设置 `TCM_CORS_ORIGINS` 为具体域名，勿长期用 `*`。
5. **LLM 容错**：未配置 Key 时系统降级 mock，仍可演示完整流程；正式对外务必配置真实模型并开启 `treatment.plan.impl: llm` 等以获得更优方案。
6. **合规**：报告与对话界面均强制展示免责声明；红旗症状路径（胸痛、咯血、高热不退等）会中断并引导线下就医，勿在生产中移除该安全 Sub-Agent。
7. **监控**：结合 `consultation.trace` 记录每个 Sub-Agent 的 impl/model/耗时，便于排查与成本统计。

## 5. 健康检查与验证

```bash
# 服务可用性
curl http://localhost:8000/api/system/agents

# 跑一次完整自测（脚本内置 mock，无需 Key）
cd backend && python smoke_test.py
```
