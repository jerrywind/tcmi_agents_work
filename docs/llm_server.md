# llm_server：可部署的本地大模型服务

`llm_server/` 为系统提供「可部署的本地大模型后端」。**也可直接采用 LM Studio / Ollama 等任意
OpenAI 兼容服务作为 LLM 后端**（本地开发更简单，无需下载 GGUF 权重），见 `deployment.md` §2.6.1。
默认包含两类模型：

- **文本模型 `qwen3.6-9B`**：承担听/问/切/辨证/安全/施治等纯文本能力。
- **视觉模型 `Qwen3-VL`**：原生多模态，承担望诊（舌象/面象/患处图片）的**图文理解**，
  **无需 mmproj**。

两类模型分别由 `llm_server`（文本）与 `llm_vision`（视觉）两个服务承载，均暴露
**OpenAI 兼容 API**，与 `backend/app/protocol/llm.py` 的 `OpenAICompatProvider` 对接，
由 backend 按模型名自动路由到对应端点。

> `OpenAICompatProvider` 同时支持两种调用协议，由 `llm.api`（或环境变量 `TCM_LLM_API`）切换：
> - `responses`（默认）：LM Studio 的 **Responses API**（`/v1/responses`），支持多模态与工具调用。
> - `chat`：传统 **Chat Completions**（`/v1/chat/completions`）。
> 本地开发最推荐用 **LM Studio + Responses API**（加载 `google/gemma-4-12b-qat` 等原生多模态模型，
> 文本与视觉共用同一端点），无需下载 GGUF 权重，见 `deployment.md` §2.6.1。

## 定位

```
前端 (Taro) ──► backend (FastAPI + Sub-Agents)
                  │
                  ├─ 文本端点 TCM_LLM_BASE_URL    ──► llm_server  (qwen3.6-9B)
                  └─ 视觉端点 TCM_LLM_VISION_BASE_URL ──► llm_vision (Qwen3-VL 原生多模态)
```

- 纯文本问诊走 `qwen3.6-9B`（文本端点）。
- 望诊图片走 `Qwen3-VL`（视觉端点），由 `Qwen3-VL` 原生理解，**不需要 mmproj**。
- 无权重/无服务时，backend 自动回退到 `MockProvider`，整套系统仍可离线运行（见 `testing.md`）。

## 目录内容

| 文件 | 作用 |
|------|------|
| `Dockerfile` | 基于 `ghcr.io/ggml-org/llama.cpp:server`，内置 `llama-server` |
| `entrypoint.sh` | 读取环境变量启动模型；`mmproj` 可选（原生多模态模型如 Qwen3-VL 无需它） |
| `.env.example` | 权重路径、端口、上下文、GPU 层数、API Key 示例 |
| `README.md` | 本服务的详细部署与 API 说明 |

## 默认模型

| 项 | 默认值 | 说明 |
|----|--------|------|
| 文本模型（llm_server） | `qwen3.6-9B` | GGUF 路径见 `MODEL_PATH`，纯文本，可选挂 mmproj |
| 视觉模型（llm_vision） | `Qwen3-VL-8B` | GGUF 路径见 `MODEL_PATH`，**原生多模态，无需 mmproj** |
| 服务端口 | `8000` | OpenAI 兼容 `/v1/chat/completions` |

> `qwen3.6-9B` / `Qwen3-VL-8B` 为项目约定的模型标识，请替换为实际可用的 Qwen GGUF。
> 视觉模型建议选用 **Qwen3-VL** 系列（原生图文理解）；若使用需投影的文本模型，
> 可另行挂载 `mmproj`。权重需自行下载，镜像**不含**任何模型文件。

## 准备权重

1. 文本模型：下载 Qwen 的 GGUF 主模型（如 `qwen3.6-9B-Q4_K_M.gguf`）放到 `llm_server/models/`。
   （可选）若文本模型需图文理解，再下载对应的 `mmproj` 投影文件放到同一目录。
2. 视觉模型：下载 **Qwen3-VL** 的 GGUF（如 `qwen3-vl-Q4_K_M.gguf`）放到 `llm_server/models/`，
   设置 `MODEL_PATH` 指向它；Qwen3-VL 内嵌视觉编码器，**无需 mmproj**。
3. 在 `.env` / compose 环境变量中确认各路径与端点。

## 本地运行（Docker）

```bash
cd llm_server
cp .env.example .env          # 按需修改路径/层数
docker build -t tcm-llm-server .
docker run --rm -p 8000:8000 --env-file .env -v "$PWD/models:/models" tcm-llm-server
```

## 通过 compose 一键拉起（含 backend）

compose 已下沉到各自目录：`llm_server/docker-compose.yml` 提供模型服务，
`backend/docker-compose.llm.yml` 提供 backend 接入本地模型的 env 覆盖。

```bash
# 1) 先在 llm_server/models 放好权重
# 2) 在 llm_server/ 启动模型服务（vision profile 同时拉起视觉服务）
cd llm_server && docker compose --profile vision up --build

# 3) 在 backend/ 用 llm 覆盖文件启动，backend 自动接入两个模型服务
cd backend && docker compose -f docker-compose.yml -f docker-compose.llm.yml --profile llm up --build
```

backend 在该配置下会自动设置：
`TCM_LLM_BASE_URL=http://llm_server:8000/v1`、`TCM_LLM_TEXT_MODEL=qwen3.6-9B`、
`TCM_LLM_VISION_MODEL=Qwen3-VL-8B`、`TCM_LLM_VISION_BASE_URL=http://llm_vision:8000/v1`、
`TCM_ROUTING_FILE=/app/routing.llm.yaml`（启用各 Sub-Agent 的 LLM 实现）。
需把两个目录的容器加入同一 docker 网络，backend 才能解析 `llm_server` / `llm_vision`。

## API 速览

| 端点 | 用途 |
|------|------|
| `GET  /health` | 服务存活（llama.cpp 自带） |
| `POST /v1/chat/completions` | OpenAI 兼容对话（支持 `tools` 与 `images` 多模态） |
| `POST /completion` | llama.cpp 原生补全 |

多模态（舌象）请求示例：

```bash
curl http://127.0.0.1:8000/v1/chat/completions \
  -H "Content-Type: application/json" -H "Authorization: Bearer sk-noauth" \
  -d '{"model":"Qwen3-VL-8B",
       "messages":[{"role":"user","content":[
         {"type":"text","text":"描述舌象"},
         {"type":"image_url","image_url":{"url":"data:image/jpeg;base64,...."}}]}]}'
```

## 调参

- `GPU_LAYERS`：CPU 填 `0`；有 CUDA 请将 `Dockerfile` 的 `FROM` 改为
  `ghcr.io/ggml-org/llama.cpp:server-cuda` 并把该项设为 `20~35` 以显存卸载加速。
- `CTX_SIZE`：上下文窗口，望诊多图时可适当调大。

详见 [`sub_agents.md`](./sub_agents.md)（各 Sub-Agent 如何调用本服务）与
[`skills.md`](./skills.md)（tcm-vision 等技能）。
