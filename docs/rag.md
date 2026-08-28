# RAG 检索服务（可选独立组件）

`llm_server/rag/` 是一套 **Python 3** 实现的 RAG 服务，覆盖三类检索：

1. **文本 RAG**：文本片段 → Embedding → 向量最近邻；
2. **图像 RAG**：图像 → 多模态 caption → Embedding → 图像向量域最近邻（以图搜图）；
3. **图文对应 RAG**：每条记录同时拥有「文本向量」与「图像向量」，可用文本或图像查询，
   跨两域联合召回（以文搜图 / 以图搜文）。

> **注意**：自 llm_server v2 起，RAG 不再是主服务的内置模块，需要时**单独启动**
> （`cd llm_server/rag && python -m rag serve`）。Embedding 端点默认指向
> `http://llm_server:8000/v1`（llm_server 网关的 `/v1/embeddings` 会透传到 LM Studio），
> 前提是 LM Studio 已加载 embedding 模型（如 `bge-m3`）。

## 架构

```
                ┌─────────────────────────── llm_server (网关/容器) ───────────────────────┐
                │                                                                          │
 文本/查询 ─────┼─► RAG 服务 (:9000) ──(text Embedding)──► /v1/embeddings ──► LM Studio    │
 图片/查询 ─────┤                                      │                                   │
                │                            (image caption)│                                 │
                │                                       ▼                                       │
                │                            /v1/chat/completions (gemma-4-12b-qat 多模态)      │
                │                                       │                                       │
                │                          caption → text Embedding → image_vec                │
                └──────────────────────────────────────────────────────────────────────────┘
                                                      │
                                                      ▼
                                            VectorStore（numpy 向量 + JSON 持久化）
```

## 模块划分

| 文件 | 职责 |
|------|------|
| `rag/config.py` | 配置与环境变量解析（端点、模型名、top_k、存储路径） |
| `rag/embedders.py` | `TextEmbedder`（OpenAI 兼容 `/v1/embeddings`）、`ImageEmbedder`（`google/gemma-4-12b-qat` 多模态生成 caption 再嵌入；与文本共用同一模型端点） |
| `rag/store.py` | `VectorStore` 与 `Record`：多模态向量存储、余弦检索、关键字回退、JSON 持久化 |
| `rag/loader.py` | 语料目录加载（图文配对 `images/<id>.jpg|+.txt` 与纯文本 `texts/`） |
| `rag/retriever.py` | `RAGService`：索引构建/增量、三种检索模式编排 |
| `rag/api.py` | FastAPI 服务与 HTTP 端点 |
| `rag/__main__.py` | CLI：`serve` / `build` / `ingest_image` |

## 数据模型

每条 `Record`：

```json
{
  "id": "img::tongue01",
  "text": "舌红苔黄腻，湿热内蕴",          // 文本描述（可选）
  "image_path": "/corpus/images/tongue01.jpg",
  "image_caption": "舌色红，苔黄腻",        // gemma-4 多模态生成的 caption（可选）
  "text_vec":  [0.01, -0.23, ...],          // 文本描述向量（用于文本/图文检索）
  "image_vec": [0.12, 0.05, ...],           // 图像 caption 向量（用于图像/图文检索）
  "meta": {"source": "...", "kind": "paired"}
}
```

- 纯文本记录只有 `text_vec`；纯图像记录只有 `image_vec`；图文配对记录两者皆有。
- 检索按 `modality` 选择向量域：`text` / `image` / `paired`（两者并集）。
- 无向量时退化为字符集合重叠的关键字匹配，保证离线可用。

## 配置（环境变量）

| 变量 | 默认 | 说明 |
|------|------|------|
| `RAG_EMBED_BASE_URL` | `http://llm_server:8000/v1` | 文本 Embedding 端点 |
| `RAG_EMBED_API_KEY` | `sk-noauth` | Embedding 端点 Key |
| `RAG_EMBED_MODEL` | `text-embedding-default` | Embedding 模型名 |
| `RAG_VISION_BASE_URL` | `http://llm_server:8000/v1` | 图像 caption 用的多模态端点（与文本共用 `google/gemma-4-12b-qat`） |
| `RAG_VISION_API_KEY` | `sk-noauth` | 视觉端点 Key |
| `RAG_VISION_MODEL` | `google/gemma-4-12b-qat` | 视觉/多模态模型名（文本与视觉共用） |
| `RAG_TOP_K` | `5` | 默认返回条数 |
| `RAG_SCORE_THRESHOLD` | `0.0` | 相似度阈值 |
| `RAG_DATA_DIR` | `/data/rag` | 索引持久化目录 |
| `RAG_CORPUS_DIR` | 无 | 预置语料目录（见下） |
| `RAG_PORT` | `8080` | 服务端口 |

## HTTP API

| 端点 | 方法 | 说明 |
|------|------|------|
| `GET /health` | GET | 存活 + 文档数 |
| `POST /rag/ingest` | POST | `{ "docs": [ {id, text?, image_path?, image_caption?, meta?} ] }` 批量入库 |
| `POST /rag/ingest_image` | POST | `{ "image_path": "...", "caption"?, "text"? }` 增量入库单图 |
| `POST /rag/build` | POST | 从 `RAG_CORPUS_DIR` 重建索引 |
| `POST /rag/retrieve/text` | POST | `{ "query": "...", "top_k"? }` |
| `POST /rag/retrieve/image` | POST | `{ "image_path": "...", "top_k"? }` |
| `POST /rag/retrieve/paired` | POST | `{ "query"?, "image_path"?, "top_k"? }` |
| `GET /rag/stats` | GET | 索引统计 |

返回示例（文本检索）：

```json
[
  {"id":"kb1","score":0.87,"modality":"text",
   "text":"风寒感冒：恶寒重发热轻，无汗...","image_path":null,"image_caption":null,"meta":{...}}
]
```

## 语料准备与索引

目录约定（`RAG_CORPUS_DIR`）：

- `texts/<name>.txt|.md`：纯文本，按空行切分；
- `images/<id>.jpg` + `images/<id>.txt`：图文配对（txt 为该图的文本描述/标签）。

```bash
# 容器内构建索引
python -m rag build

# 或增量：HTTP 入库
curl -X POST http://localhost:8080/rag/ingest \
  -H "Content-Type: application/json" \
  -d '{"docs":[{"id":"kb1","text":"风寒感冒：恶寒重发热轻，无汗..."}]}'
```

## 与 Sub-Agent 的衔接（可选）

后端可经环境变量指向本服务：
- **harness（Rust，现行）**：`HARNESS_RAG_ENDPOINT=http://<rag-host>:<port>`
  （前缀是 `HARNESS_`；留空时 `tcm-rag` 技能返回提示而不报错）
- **原 backend（Python，已归档）**：`TCM_RAG_BASE_URL=http://llm_server:8080`

Sub-Agent 在执行时若需检索药典、医案、舌象图谱等资料，可调用 RAG 服务获得相关上下文后
再生成结论；无 RAG 服务时不影响问诊流程（参见 `docs/sub_agents.md`、`docs/skills.md`）。

## 运行测试

```bash
cd llm_server/rag
pip install numpy fastapi httpx     # 运行期/测试依赖
python -m unittest test_rag -v       # 离线测试（文本/图像/图文检索、持久化、降级）
```

> 测试无需模型服务：Embedding 与视觉端点不可达时会降级为零向量或关键字匹配，
> 重点验证索引、检索与持久化逻辑。
