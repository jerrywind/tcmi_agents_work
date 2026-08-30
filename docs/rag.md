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
| `rag/corpus.py` | **中医典籍全文检索**（T4.3）：切条目 + bigram 倒排 + BM25 书级召回 + 片段重排，**不依赖 Embedding** |
| `rag/eval_rag.py` | 召回质量评分（T4.3）：读 `eval/tcm_queries.jsonl` 跑分，输出 hit@k / MRR / 耗时 |
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

harness 经环境变量 `HARNESS_RAG_ENDPOINT` 指向本服务（前缀是 `HARNESS_`；
留空时 `tcm-rag` 技能返回提示串而不报错）。

> 契约已对齐（T2.1）：`HARNESS_RAG_ENDPOINT` 需填写到具体端点路径，例如
> `http://<rag-host>:8080/rag/retrieve/text`；`tcm-rag` 技能 POST
> `{"query": "...", "top_k"?: N}`，并把响应统一包成 `{"result": [...]}`。

Sub-Agent 在执行时若需检索药典、医案、舌象图谱等资料，可调用 RAG 服务获得相关上下文后
再生成结论；无 RAG 服务时不影响问诊流程（参见 `docs/sub_agents.md`、`docs/skills.md`）。

## 典籍全文检索（T4.3）

向量 RAG 需要 embedding 模型，本机常常没有；而**中医典籍检索主要靠字面匹配**
（方名、药名、条文），字面检索快、稳、可离线复现。故新增 `corpus.py`：

```
《xxx》按 <篇名> 切条目 ──► 汉字 bigram 倒排（SQLite）
        │                        │
        │                        ├─ 书级 BM25：先选出最可能包含答案的几部书
        │                        └─ 片段级重排：IDF 覆盖 + 整词加权 + 同书限流
        └─ 条目标题（如「甘草」）拼进正文：典籍正文常常通篇不提自己的名字
```

```bash
cd llm_server
# 建库（700 部 / 6618 万字，约 60s）
python -m rag corpus-build --dir ../rag_data --db ../rag_data/_index/corpus.sqlite3
# 检索（平均 41ms）
python -m rag corpus-search --db ../rag_data/_index/corpus.sqlite3 --query "半夏泻心汤 心下痞"
# 召回质量跑分
python -m rag eval --queries rag/eval/tcm_queries.jsonl \
    --db ../rag_data/_index/corpus.sqlite3 --top-k 5 --top-docs 3
```

关键设计（都是踩过坑才加的）：

| 设计 | 原因 |
|---|---|
| 编码探测（GB18030 优先） | 语料实际是 GB18030；按 utf-8 读会**静默丢字**，表现为「索引建好却什么都搜不到」 |
| 剥离书目元数据头 | `<篇名>书名 / 作者 / 朝代 / 年份` 不是医学内容，否则「张仲景」「东汉」会把无关查询吸过来 |
| 条目标题拼进正文 | 药名/方名条目正文常不提自己的名字（「味甘平，主咳逆上气…」） |
| 同书限流（`per_doc`） | 《普济方》这类巨著命中片段极多，不设上限会把 top-k 占满 |
| 整词加权 | 「半夏泻心汤」整词命中必须压过零散 bigram 命中 |
| 路径存绝对路径 | 检索时要回读原文，相对路径换目录就找不到文件 |

**评估判据是「原文原样是否被召回」而不是「出自哪本书」**：语料 694 部，
同一张经方在多部典籍里都有论述，「该出自哪本」没有唯一答案，按书名打分会把
正确答案判错。样例集 `eval/tcm_queries.jsonl`（24 条，每条标注应当出现的原文
字样与依据），基线：**hit@5 95.8% / hit@1 95.8% / MRR 0.958 / 平均 41ms**，
报告存 `eval/baseline.json`。

已知不足：带常见修饰词的复合查询（如「妊娠禁忌候 孕妇起居饮食宜忌」）会被
高频词带偏，罕见的那个词反而排不上——纯字面检索的通病，根治要靠 embedding 重排。

## 运行测试

```bash
cd llm_server/rag
pip install numpy fastapi httpx     # 运行期/测试依赖
python -m unittest test_rag -v       # 检索服务（文本/图像/图文、持久化、降级）
python -m unittest test_corpus -v    # 典籍索引（切分/编码/建库/脱敏/评估，纯离线 12 条）
```

> 测试无需模型服务：Embedding 与视觉端点不可达时会降级为零向量或关键字匹配，
> 重点验证索引、检索与持久化逻辑。`test_corpus` 全程不联网。
