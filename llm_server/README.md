# llm_server —— 可部署的本地大模型服务

为项目提供「可部署的本地大模型后端」，默认包含两类模型：

- **文本模型 `qwen3.6-9B`**：承担听/问/切/辨证/安全/施治等纯文本能力。
- **视觉模型 `Qwen3-VL`**：原生多模态，承担望诊（舌象/面象/患处图片）的**图文理解**，
  **无需 mmproj**。

两类模型分别由 `llm_server`（文本）与 `llm_vision`（视觉）两个服务承载，均暴露
**OpenAI 兼容 API**，与 `backend/app/protocol/llm.py` 的 `OpenAICompatProvider` 对接，
由 backend 按模型名自动路由到对应端点。

## 架构定位

```
前端 (Taro)  ──►  backend (FastAPI + sub-agents)
                  │
                  ├─ 文本端点 TCM_LLM_BASE_URL      ──► llm_server (qwen3.6-9B)
                  └─ 视觉端点 TCM_LLM_VISION_BASE_URL ──► llm_vision (Qwen3-VL 原生多模态)
```

- backend 文本端点 `TCM_LLM_BASE_URL` 指向 `llm_server`（容器内 `http://llm_server:8000/v1`）。
- 视觉端点 `TCM_LLM_VISION_BASE_URL` 指向 `llm_vision`（容器内 `http://llm_vision:8000/v1`）。
- 纯文本问诊走 `qwen3.6-9B`；望诊图片走 `Qwen3-VL`，由原生多模态理解，**无需 mmproj**。
- 无模型可用时，backend 自动回退到 `MockProvider`，整套系统仍可离线运行（见 `docs/testing.md`）。

## 默认模型

| 项 | 默认值 | 说明 |
|----|--------|------|
| 文本模型（llm_server） | `qwen3.6-9B` | GGUF 路径见 `MODEL_PATH`，纯文本，可选挂 mmproj |
| 视觉模型（llm_vision） | `Qwen3-VL-8B` | GGUF 路径见 `MODEL_PATH`，**原生多模态，无需 mmproj** |
| 服务端口 | `8000` | OpenAI 兼容 `/v1/chat/completions` |

> 说明：`qwen3.6-9B` / `Qwen3-VL-8B` 为项目约定的模型标识，请替换为实际可用的 Qwen GGUF。
> 视觉模型建议选用 **Qwen3-VL** 系列（原生图文理解）；若使用需投影的文本模型，可另行挂载 mmproj。
> 权重需自行下载，镜像**不含**任何模型文件。

## 准备权重

1. 文本模型：下载 Qwen 的 GGUF 主模型（如 `qwen3.6-9B-Q4_K_M.gguf`）放到 `llm_server/models/`；
   （可选）若文本模型需图文理解，再下载对应 `mmproj` 投影文件放到同一目录。
2. 视觉模型：下载 **Qwen3-VL** 的 GGUF（如 `qwen3-vl-Q4_K_M.gguf`）放到 `llm_server/models/`，
   设置 `MODEL_PATH` 指向它；Qwen3-VL 内嵌视觉编码器，**无需 mmproj**。
3. 在 `.env` / compose 环境变量中确认各路径与端点。

## 本地运行（Docker）

```bash
cd llm_server
cp .env.example .env          # 按需修改路径/层数；视觉服务单独配置 MODEL_PATH=qwen3-vl
docker build -t tcm-llm-server .
# 文本服务
docker run --rm -p 8001:8000 --env-file .env -v "$PWD/models:/models" tcm-llm-server
# 视觉服务（另起一个终端，指向 Qwen3-VL）
MODEL_PATH=/models/qwen3-vl.gguf docker run --rm -p 8002:8000 --env-file .env -v "$PWD/models:/models" tcm-llm-server
```

## 通过 compose 一键拉起（与 backend 联调）

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
`TCM_ROUTING_FILE=/app/routing.llm.yaml`（启用各 sub-agent 的 LLM 实现）。
需把两个目录的容器加入同一 docker 网络，backend 才能解析 `llm_server` / `llm_vision`。

## API 速览

| 端点 | 用途 |
|------|------|
| `GET  /health` | 服务存活（llama.cpp 自带） |
| `POST /v1/chat/completions` | OpenAI 兼容对话（支持 `tools` 与 `images` 多模态） |
| `POST /completion` | llama.cpp 原生补全 |

文本请求示例（qwen3.6-9B）：

```bash
curl http://127.0.0.1:8001/v1/chat/completions \
  -H "Content-Type: application/json" -H "Authorization: Bearer sk-noauth" \
  -d '{"model":"qwen3.6-9B","messages":[{"role":"user","content":"你好"}],"temperature":0.3}'
```

多模态（舌象）请求示例（Qwen3-VL）：

```bash
curl http://127.0.0.1:8002/v1/chat/completions \
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

## RAG 服务（文本 / 图像 / 图文对应检索）

镜像内置一套 **Python 3** 实现的 RAG 服务（源码见 `llm_server/rag/`），由 `entrypoint.sh`
在启动模型的同时于后台拉起（端口 `RAG_PORT`，默认 8080）。它**复用**本服务的
Embedding 端点与 `llm_vision` 的 Qwen3-VL 端点，无需额外模型。

三种检索模式：

| 模式 | 端点 | 说明 |
|------|------|------|
| 文本 RAG | `POST /rag/retrieve/text` | 文本 → Embedding → 文本向量域最近邻 |
| 图像 RAG | `POST /rag/retrieve/image` | 图像 → Qwen3-VL caption → Embedding → 图像向量域最近邻（以图搜图） |
| 图文对应 RAG | `POST /rag/retrieve/paired` | 可用「文本」或「图像」查询，跨 text_vec / image_vec 两域联合召回（以文搜图 / 以图搜文） |

索引结构：每条记录可同时持有 `text_vec`（文本描述向量）与 `image_vec`（图像 caption 向量），
因此同一份图文资料能被两种模态互相检索。向量用余弦相似度，无向量时回退到字符重叠关键字匹配。
向量存储在 `RAG_DATA_DIR`（默认 `/data/rag`，compose 中已挂持久卷）。

### 语料准备

将资料放入 `RAG_CORPUS_DIR`（compose 中映射到 `./llm_server/corpus`）：

- `texts/<name>.txt|.md`：纯文本片段，按空行切分；
- `images/<id>.jpg` + `images/<id>.txt`：图文配对（txt 为该图文本描述/标签）。

然后执行索引构建：

```bash
# 容器内
python -m rag build
```

或者直接调用 HTTP 接口增量入库：

```bash
curl -X POST http://localhost:8080/rag/ingest \
  -H "Content-Type: application/json" \
  -d '{"docs":[{"id":"kb1","text":"风寒感冒：恶寒重发热轻，无汗..."}]}'

# 以图入库（caption 可由 Qwen3-VL 自动生成，也可自带）
curl -X POST http://localhost:8080/rag/ingest_image \
  -H "Content-Type: application/json" \
  -d '{"image_path":"/corpus/images/tongue01.jpg","text":"舌红苔黄腻"}'
```

### 检索示例

```bash
# 文本 RAG
curl -X POST http://localhost:8080/rag/retrieve/text \
  -H "Content-Type: application/json" -d '{"query":"恶寒重发热轻无汗","top_k":3}'

# 图像 RAG（以图搜图）
curl -X POST http://localhost:8080/rag/retrieve/image \
  -H "Content-Type: application/json" -d '{"image_path":"/corpus/images/tongue01.jpg"}'

# 图文对应 RAG（以文搜图）
curl -X POST http://localhost:8080/rag/retrieve/paired \
  -H "Content-Type: application/json" -d '{"query":"舌红苔黄腻"}'
```

完整设计与 API 见 `docs/rag.md`。

详见 `docs/llm_server.md` 与 `docs/sub_agents.md`。
