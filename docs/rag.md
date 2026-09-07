# RAG 检索服务（可选独立组件）

`llm_server/rag/` 是一套 **Python 3** 实现的 RAG 服务，覆盖三类检索：

1. **文本 RAG**：文本片段 → Embedding → 向量最近邻；
2. **图像 RAG**：图像 → 多模态 caption → Embedding → 图像向量域最近邻（以图搜图）；
3. **图文对应 RAG**：每条记录同时拥有「文本向量」与「图像向量」，可用文本或图像查询，
   跨两域联合召回（以文搜图 / 以图搜文）。

> **运行形态**：RAG 有两条路，按需取用——
>
> 1. **随 llm_server 主服务挂载（默认，推荐）**：`app/rag_router.py` 把 RAG 路由
>    并进主应用，端点即 `http://<llm_server>:8000/rag/*`。这也是 harness
>    `rag_endpoint` 默认指向的位置——此前主服务没挂 RAG，导致整条链路空转。
> 2. **独立服务**：`cd llm_server/rag && python -m rag serve`（默认端口 **8080**）。
>
> 两种形态都可在索引缺失/依赖不全时**优雅降级**：主服务里 `/rag/health` 返回
> 503 + 原因，其余 `/rag/*` 报 503，网关本身不受影响。
> Embedding 端点默认指向 `http://llm_server:8000/v1`，前提是 LM Studio 已加载
> embedding 模型（如 `bge-m3`）；典籍检索走离线 bigram 索引，**不依赖 Embedding**。

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
| `rag/corpus.py` | **中医典籍全文检索**（T4.3）：切条目 + bigram 倒排 + BM25 书级召回 + 片段重排，**不依赖 Embedding**；另含 `doc_tags` 分类标签表与按标签过滤检索 |
| `rag/taxonomy.py` | **典籍多标签分类**（见下）：从书名 + 头部元数据打「临床学科 / 内容体裁 / 学术流派」三维标签 |
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
| `RAG_CORPUS_DB` | `/data/rag/corpus.sqlite3` | **典籍倒排索引库路径**（预建索引在 `rag_data/_index/corpus.sqlite3`，挂载后须指向它） |
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
| **`POST /rag/retrieve/scope`** | POST | **按知识域检索典籍**（sub-agent 专用），见下 |
| `GET /rag/stats` | GET | 索引统计 |
| `GET /rag/tags?dim=` | GET | 列出可用标签；`dim` 指定维度 |

### `POST /rag/retrieve/scope`：按知识域检索

每个 sub-agent 有自己该看的书（开方只看方书、切诊只看脉学），用四维分类标签
圈定检索范围。**维度之间取交集**——「方书 AND 儿科」，而不是「方书 OR 儿科」。

```jsonc
// POST /rag/retrieve/scope
{
  "query": "小儿发热咳嗽",
  "top_k": 3,
  "genres": ["方书方剂"],        // 内容体裁
  "functions": [],               // 功能用途
  "departments": ["儿科"],       // 临床学科（由辨证结果动态注入）
  "schools": [],                 // 学术流派（默认留空，避免学术偏见）
  "require_all": true            // true=跨维度交集；false=全部并集
}
```

实测效果：

| 查询 | 限定 | 命中 |
|---|---|---|
| 小儿发热咳嗽 | 方书 ∩ 儿科 | 《慈幼便览》《小儿卫生总微论方》 |
| 妊娠恶阻 | 方书 ∩ 产科 | 《妇人大全良方·妊娠恶阻方论第二》 |
| 脉象浮数 | 诊断学 | 《脉诀汇辨》《脉诀阐微》 |
| 脉象浮数 | 本草药物 | 空（本草书不谈脉象） |

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

harness 经 `rag_endpoint`（环境变量 `HARNESS_RAG_ENDPOINT`）指向本服务。
`resources/config.yaml` 已默认启用，指向 llm_server 主服务：

```yaml
rag_endpoint: "http://llm_server:8000/rag/retrieve/text"   # docker 同一网络
# 本地开发：http://localhost:8000/rag/retrieve/text
```

> 契约已对齐：`rag_endpoint` 需填到具体端点路径；`tcm-rag` 技能 POST
> `{"query": "...", "top_k"?: N}`，并把响应统一包成 `{"result": [...]}`。
> **服务不可达时优雅降级**（返回 `{"error": ...}`），不会中断问诊流程。

Sub-Agent 在执行时若需检索药典、医案、舌象图谱等资料，可调用 RAG 服务获得相关上下文后
再生成结论；无 RAG 服务时不影响问诊流程（参见 `docs/sub_agents.md`、`docs/skills.md`）。

### 启用前提：语料必须挂进 llm_server 容器

RAG 端点虽然一直挂在 llm_server 主应用上，但**语料（`rag_data/`）从未挂载进容器**，
容器内的 `corpus_db` 默认是空路径 `/data/rag/corpus.sqlite3`，于是索引为 `None`、
`tcm-rag` 永远检索不到任何典籍——链路空转。启用需要三件事齐备：

1. **挂载语料卷**：把仓库 `rag_data/`（694 部 txt 原文 + `_index/corpus.sqlite3` 预建索引）
   挂到容器 `/data/rag`。两处 compose 均已配好：
   - 生产：`deploy/docker-compose.yml` 的 `llm_server` 服务（同时为 harness 的
     `rag_endpoint` 提供可达的服务名 `llm_server`）；
   - 本地联调：`llm_server/docker-compose.yml`。
2. **环境变量**（与挂载配套，见上表）：
   ```yaml
   RAG_DATA_DIR: "/data/rag"
   RAG_CORPUS_DIR: "/data/rag"            # 语料根：原文回读的候选目录
   RAG_CORPUS_DB: "/data/rag/_index/corpus.sqlite3"
   ```
3. **预建索引**：`rag_data/_index/corpus.sqlite3` 由宿主机离线生成
   （`corpus-build` + `corpus-classify`，约一分钟，命令见「典籍全文检索」小节），
   无需在容器内重建。

> 索引内 `docs.path` 记录的是**建库机的绝对路径**（宿主机 Windows 的 `D:\...`），
> 容器里路径不同；`CorpusIndex` 已做路径可移植处理——记录路径失效时按
> 「语料根目录 + 文件名」回退（`corpus.py::_resolve_text_path`），因此宿主机建的
> 索引可直接跨机/进容器使用，个别原文缺失也只跳过该本、不中断检索。

**验证**：llm_server 起来后看启动日志 `RAG 已挂载：corpus_db=... top_k=...`；
`curl http://127.0.0.1:22010/rag/stats`（本地联调）应返回
`"corpus": {"books": 696, "terms": ..., "postings": ...}` 而不是 `null`；
再 `POST /rag/retrieve/text {"query": "半夏泻心汤 心下痞"}` 应能返回《类证治裁》等典籍片段
（实测命中「心下痞，发热而呕，半夏泻心汤」）。
harness 侧 `GET /health` 的 `rag.reachable` 应为 `true`（后台每 60s 探测一次）。

> ⚠️ **验证时一律用 `127.0.0.1`，不要用 `localhost`**：Windows 上 `localhost`
> 会先解析到 IPv6 `::1`，容器未监听时回落 IPv4 要等约 **21 秒**。
> 这个延迟会把所有耗时测量污染掉（实测同一个请求：`localhost` 29.12s /
> `127.0.0.1` 0.14s），足以让人得出完全错误的结论。

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

## 典籍多标签分类

694 部典籍只按书名检索，下面这些需求天然做不了：

- 「只看儿科书」——《幼科铁镜》《证治准绳·幼科》《婴童百问》正文里未必有「儿科」二字；
- 「只看医案」——《临证指南医案》与《洄溪医案》在字面上毫无交集；
- 「只看火神派」——流派是**人**（郑钦安、吴佩衡）的传承，书里不会自称「火神派」。

故按**文件**（书名 + 头部元数据）做规则化多标签分类，四个维度正交打标：

| 维度 | 说明 | 取值（部数，694 部语料） |
|---|---|---|
| `临床学科` | 相当于现代医院科室 | 全科综合 260 / 内科 84 / 本草方药 58 / 基础理论 45 / 儿科 43 / 针灸推拿 43 / 外科 38 / 妇科 30 / 温病疫病 21 / 骨伤科 19 / 养生食疗 18 / 产科 17 / 耳鼻喉口齿 15 / 眼科 12 / 医史文献 2 / 法医学 1 |
| `内容体裁` | 这本书是什么类型的著作 | 临床各科 215 / 方书方剂 110 / 临床综合 96 / 本草药物 58 / 医话医论 54 / 临证医案 48 / 伤寒论 47 / 针灸经络 44 / 医经基础 42 / 入门歌诀 24 / 诊断学 23 / 养生摄生 19 / 金匮要略 11 / 医史文献 2 |
| `功能用途` | 这本书「拿来干什么」 | 专科证治 172 / 综合证治 73 / 医论争鸣 54 / 临证实录 47 / 经典诠释 33 / 方剂汇编 32 / 经典注疏 29 / 专科方书 25 / 理论建构 24 / 诊断方法 23 / 歌诀便诵 22 / 经验验方 22 / 方论阐释 18 / 药性理论 17 / 养生调摄 16 / 针灸综合 15 / 本草入门 12 / 本草集解 10 / 经典本草 10 / 刺法灸法 9 / 腧穴考证 8 / 经络理论 7 / 急救方书 5 / 炮制制剂 5 / 配伍归经 5 / 食疗本草 5 / 推拿按摩 4 / 入门启蒙 2 / 成药标准 2 / 文献考据 2 / 时间针法 2 / 药物图谱 2 |
| `学术流派` | 师承与主张 | 伤寒派 85 / 温热派 43 / 医经派 37 / 温补派 26 / 滋阴派 14 / 易水派 10 / 河间派 9 / 中西汇通派 5 / 火神派 5 / 攻邪派 1 |

一部书在每个维度可以有**多个**标签（《妇人大全良方》= 妇科 + 产科；《范中林六经辨证医案》
= 伤寒派 + 火神派），也可以打不上（落 `未归类`），绝不硬塞。

### 功能用途：本草 / 方书 / 针灸专著的细分

体裁只回答「是什么」，功能回答「拿来干什么」——同一体裁内部的用处可能天差地别：

| 体裁 | 按功能细分 |
|---|---|
| 本草药物（58） | 药性理论 16（本草衍义、汤液本草）/ 本草入门 12（本草备要、药性赋）/ 经典本草 10（神农本草经、名医别录）/ 本草集解 10（本草纲目、证类本草）/ 配伍归经 5（得配本草、药症忌宜）/ 炮制制剂 5（雷公炮炙论、炮炙大法）/ 食疗本草 / 药物图谱 |
| 方书方剂（110） | 方剂汇编 32（普济方、苏沈良方）/ 专科方书 25（妇人大全良方、外科集验方）/ 方论阐释 18（医方考、古今名医方论）/ 经验验方 22（验方新编、串雅内外编）/ 歌诀便诵（汤头歌诀、时方歌括）/ 急救方书 5（肘后备急方、急救良方）/ 成药标准 2（太平惠民和剂局方） |
| 针灸经络（44） | 针灸综合 15（针灸大成、针灸资生经）/ 刺法灸法 9（宋本备急灸法、神灸经纶）/ 腧穴考证 8（经穴汇解、凌门传授铜人指穴）/ 经络理论 7（十四经发挥、奇经八脉考）/ 推拿按摩 4（厘正按摩要术、小儿推拿广意）/ 时间针法 2（子午流注针经） |

效果上，查「附子炮制」不加标签会掉进《博济方》，限定 `--tags 炮制制剂` 才落到《炮炙全书》；
查「灸法」限定 `--tags 刺法灸法` 落到《宋本备急灸法》《灸法秘传》。

没命中具体功能的按体裁兜底（《伤寒论》→ 经典诠释，《临证指南医案》→ 临证实录），
所以每部书都有功能标签。

```bash
cd llm_server
# 全量分类：出 JSON + Markdown 报告，并把标签写进索引库 doc_tags 表（约 3 秒）
python -m rag corpus-classify --dir ../rag_data --db ../rag_data/_index/corpus.sqlite3
# 只看标签分布
python -m rag corpus-tags --db ../rag_data/_index/corpus.sqlite3
# 按标签收窄检索范围（同一维度多标签取并集；跨维度不会自动求交）
python -m rag corpus-search --query "小儿发热咳嗽" --tags 儿科
python -m rag corpus-search --query "附子炮制" --tags 炮制制剂
python -m rag corpus-search --query "妊娠恶阻" --tags 专科方书
```

跨维度交集用 Python API / HTTP 端点：

```python
idx.search("小儿发热咳嗽", tag_groups=[["方书方剂"], ["儿科"]])   # 方书 AND 儿科
```

产物：

| 文件 | 内容 |
|---|---|
| `rag_data/_index/classification.json` | 全量机器可读结果（doc_id / 书名 / 作者 / 朝代 / 时代 / 三维标签） |
| `rag_data/_index/classification.md` | 按标签分组的人读报告 |
| 语料库 `doc_tags(doc_id, dim, tag)` | 检索时按标签过滤用；`doc_meta` 存作者/朝代/时代 |

Python API：`CorpusIndex.search(query, tags=["儿科"])`。

### 为什么是规则而不是模型

| 决定 | 原因 |
|---|---|
| 以书名为主要信号 | 书名是作者原题，信息密度极高——「幼科」「疡医」「胎产」本身就是分类信号 |
| 流派由**作者**判定 | 郑钦安 → 火神派、叶天士 → 温热派，比正文关键词稳定得多 |
| 体裁兜底科室 | 《医方考》这类方书本身不分科，书名里没有科室线索，落到「全科综合」而不是漏标 |
| 功能靠体裁兜底 | 没命中具体功能的按体裁兜底（《针灸大成》→ 针灸综合），保证每部书都有功能标签 |
| 专科方书要预计算 | 检索端多标签是**并集**语义，没法用「妇科 + 方书方剂」取交集，故把「某科 ∩ 方书」单独做成标签 |
| 专科书不套流派 | 流派是辨证论治的主张，《伤科补要》《审视瑶函》这类书不谈流派；语料里作者字段偶有张冠李戴（《伤科补要》的作者被标成钱潢），不设闸就会给它挂上「伤寒派」 |
| 繁体先转简体 | `651-十二經補瀉溫涼引經藥歌` 这类书名，不转换会整条规则漏掉 |
| 按标签检索时不做泛词抑制 | df 是按全库算的：「附子」在 696 部里出现 525 部，按全库标准是泛词；但候选已被标签收窄到 5 部火神派书时，它恰恰是最该命中的词 |

### 已知不足

- **流派维度约七成落在「未归类」**：本草、方书、针灸、眼科等专著本身不入流派，
  属正常；另一部分是因为「中医瑰宝苑」导出格式的典籍没有作者元数据，无从判定。
- 规则按书名/作者匹配，语料元数据出错（作者张冠李戴、字段串位）会传导为误标；
  发现后可直接改 `taxonomy.py` 的规则表重跑，无需重建索引。

## 检索耗时、预热与探活

「平均 41ms」是 **CLI 在已预热样例上** 的数字，别直接拿来当线上预算。
容器内实测（71MB 倒排库）：

| 阶段 | 耗时 | 说明 |
|---|---|---|
| 预热后 / 重复查询 | 0.05–0.14s | 常态 |
| 新查询首次检索 | ~3.5s | postings 冷读盘，同一查询再查即回落 |
| embedding 端点不可达或不可解析 | ~8s/次 | **空向量库时本可完全避免** |

由此做了两处改进，都是被「探活误报」逼出来的：

1. **向量库为空时跳过 embedding**（`retriever.py`）：库里一条向量都没有，
   嵌入结果无处可用；而离线部署的常态就是端点不可达，每请求白付数秒。
   修完冷启动首查 **29.12s → 0.14s**。
2. **启动后后台预热**（`RAGService.warmup`，在 `app/main.py` 的 lifespan 里
   `asyncio.create_task`）。预热用的查询与 harness 探活的 `query` **必须是同一个
   字符串**（`健康检查`，见 `retriever.py::WARMUP_QUERY` 与 `rag_health.rs::probe`）：
   预热的页和探活要读的页不是一批的话，预热等于白做。

harness 侧 `PROBE_TIMEOUT` 由 5s 放宽到 **30s**：5s 会把「慢」判成「挂」——
每次部署重启后都有一分钟报「典籍不可用」，而服务其实是好的。探测是 60s 一轮的
后台任务，放宽不会影响任何请求延迟。

修复后实测：llm_server 监听后 **40ms** 即 `rag.reachable=true`；
此前第一次探活必失败，要等满 60s 的第二轮才转 true。

## 运行测试

```bash
cd llm_server/rag
pip install numpy fastapi httpx     # 运行期/测试依赖
python -m unittest test_rag -v       # 检索服务（文本/图像/图文、持久化、降级）
python -m unittest test_corpus -v    # 典籍索引（切分/编码/建库/脱敏/评估/标签过滤，纯离线）
python -m unittest test_api_scope -v # 知识域 scope 编译语义（纯离线，不依赖索引）
python -m unittest test_taxonomy -v  # 典籍分类（四维多标签/繁体/作者流派/标签过滤，纯离线 27 条）
```

> 测试无需模型服务：Embedding 与视觉端点不可达时会降级为零向量或关键字匹配，
> 重点验证索引、检索与持久化逻辑。`test_corpus` 全程不联网。
