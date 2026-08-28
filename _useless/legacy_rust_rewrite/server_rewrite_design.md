# server/ 全量重写设计文档

> 状态：**设计阶段（尚未实现）**
> 目标：把现有 `backend/`（Python/FastAPI 中医诊疗编排）与 `rrserver/`（Rust 中继 + 隧道）合并为一个 **Rust 实现的 `server/`**，
> 同时保留 rrserver 的中继/隧道能力，并让家庭端 `llm_server` 经 WS 隧道注册进来，新 `server` 通过 `/t/<name>/v1` 访问家庭端 LM Studio 部署的模型。
> 本文件只定义架构、模块边界、接口契约与 Rust 模块签名（**不含实现**），确认后再落地代码。

---

## 1. 目标与范围

### 1.1 要做什么
- 新建 `server/`（单个 Rust crate，或 cargo workspace），承载：
  1. **诊断编排**（原 `backend`）：7 个 Sub-Agent、诊断 Loop、报告、家庭档案、PPG、流式分段、MCP、skills。
  2. **中继/隧道**（原 `rrserver`）：云端 `server` 注册接口、`/ws/<name>` 控制连接、`/t/<name>/*` 反代、家庭端 `client`、`llm_server` 注册包装。
- 所有 nginx/TLS 仍由 `deploy/` 统一负责（见 `deploy/nginx/`），`server/` 自身只暴露 HTTP（容器内 `8080`）。

### 1.2 不做什么（明确排除）
- 不重写前端（`frontend/`）。
- 不重写 `llm_server` 家庭端包装（它已是 Rust，仅做"部署+注册"，逻辑几乎不变，仅需与新 `server` 的 `[[tunnels]]` 凭据对接）。
- 不在 `server/` 内放 nginx。

### 1.3 关键拓扑（确认项）
```
                 ┌─────────────── deploy/nginx (:443 TLS) ─────────────┐
浏览器/小程序 ───►│  /rr/t/<name>/*  ──►  server(:8080)                │
                 │  /api/*          ──►  server(:8080) 诊断编排          │
                 └──────────────────────────────────────────────────────┘
                                          │ 经隧道 /t/home/v1
                                          ▼
                          家庭端 llm_server ──► 本地 LM Studio(:11223)

llm_server 在家庭端：
  1. 启动/接入本地 LM Studio
  2. 用 name+token 向 server 的 /api/register 注册，建立 WS 控制连接
  3. server 收到 /t/home/v1/chat/completions 请求 → 通过 WS 隧道转发到家庭端 → llm_server → LM Studio
```
- **SSE 流式已支持**：现有 rrserver 隧道（`rrserver/src/client.rs` 的 `forward_to_ws` 用 `r.chunk()` 逐块回传；`server.rs` 用 `Body::from_stream` 流式回吐）**已原生支持 SSE 流式透传**，无需改造隧道协议即可保留"边说边出"。设计文档默认复用该机制。

---

## 2. 目录结构（目标态）

```
server/
├── Cargo.toml                 # 单 crate；或 workspace（server + lib）
├── Dockerfile
├── config/
│   ├── server.toml.example    # 合并：中继 tunnels + 诊断 settings
│   └── routing.toml.example   # capability → impl/model 路由（替代 routing.yaml）
├── src/
│   ├── main.rs                # 入口：解析配置、启动 HTTP、注册 routing
│   ├── config.rs              # 配置结构（Settings）
│   ├── error.rs               # 统一错误 + 转 HTTP
│   │
│   ├── relay/                 # 复用 rrserver 中继能力（原 rrserver/src）
│   │   ├── mod.rs
│   │   ├── server.rs          # 注册 / WS / /t/<name> 反代（含 SSE 流式）
│   │   ├── client.rs          # 家庭端隧道客户端（含 forward_to_ws）
│   │   ├── llmsrv.rs          # llm_server 部署+注册包装（家庭端用）
│   │   ├── protocol.rs        # WS 消息协议（Request/Response/Chunk）
│   │   ├── state.rs           # 隧道注册表（Arc<RwLock<...>>）
│   │   └── skill.rs           # 远端能力公告（可选）
│   │
│   ├── diagnose/              # 原 backend 诊断编排（核心）
│   │   ├── mod.rs
│   │   ├── orchestrator.rs    # 诊断 Loop（start/start_sync/answer/rounds/report）
│   │   ├── agent.rs           # Sub-Agent 抽象 + 调用分发
│   │   ├── capability.rs      # Capability 枚举 + routing 解析
│   │   ├── routing.rs         # routing.toml 加载与查询
│   │   ├── registry.rs        # impl 注册表（rule / llm / mcp）
│   │   ├── report.rs          # 报告 / 方案 / 待办 / 随访 生成
│   │   ├── stream.rs          # 流式分段（seq 游标 + 后台任务）
│   │   └── ppg.rs             # PPG 脉象解析（移植 knowledge/ppg.py）
│   │
│   ├── agents/                # 7 个 Sub-Agent 实现（移植 backend/app/agents）
│   │   ├── mod.rs
│   │   ├── inspection.rs      # 望诊（vision，多模态图片）
│   │   ├── auscultation.rs    # 闻诊（rule/文本）
│   │   ├── interrogation.rs   # 问诊（llm 提问+归类）
│   │   ├── palpation.rs       # 切诊（消费 ppg 证据 + llm）
│   │   ├── differentiation.rs # 辨证（llm rule-following）
│   │   ├── knowledge.rs       # 知识检索（syndromes/question_bank/advice）
│   │   └── safety.rs          # 用药安全硬校验（herb_safety.py）
│   │
│   ├── knowledge/             # 知识库数据（移植 backend/app/knowledge）
│   │   ├── mod.rs
│   │   ├── syndromes.rs       # 证候→推理规则 / 方剂 / 建议库
│   │   ├── question_bank.rs   # 辨证问卷
│   │   ├── advice.rs          # 生活/膳食调护
│   │   └── herb_safety.rs     # 十八反/十九畏/妊娠禁忌
│   │
│   ├── llm/                   # LLM 调用层（OpenAI 兼容，可走隧道）
│   │   ├── mod.rs
│   │   ├── client.rs          # 同步/流式 chat/responses；base_url 可指向隧道
│   │   └── types.rs           # 请求/响应/工具调用结构
│   │
│   ├── mcp/                   # MCP Server + Client（移植 backend/app/mcp）
│   │   ├── mod.rs
│   │   ├── server.rs          # Streamable HTTP server
│   │   ├── client.rs          # 外部 MCP 连接池
│   │   └── tools.rs           # agent/session 工具暴露
│   │
│   ├── skills/                # 技能热装载（移植 backend/app/skills）
│   │   ├── mod.rs
│   │   ├── loader.rs
│   │   └── registry.rs
│   │
│   ├── store/                 # 持久化（原 backend/app/store.py）
│   │   ├── mod.rs             # trait ConsultationStore / FamilyStore
│   │   ├── memory.rs          # 默认内存实现（HashMap，带 TTL 可选）
│   │   └── redis.rs           # 可选 redis 实现（feature gate）
│   │
│   ├── models/                # 领域模型（等价 Pydantic，见 §4）
│   │   ├── mod.rs
│   │   ├── archive.rs         # Patient/Family/Member
│   │   ├── consultation.rs    # Consultation + 子结构
│   │   ├── evidence.rs        # Evidence/Hypothesis
│   │   └── report.rs          # Report/TreatmentPlan/CareTodo/FollowUp
│   │
│   └── api/                   # REST 路由（对齐 backend 契约，见 §3）
│       ├── mod.rs             # 挂载全部 router
│       ├── consultations.rs
│       ├── families.rs
│       ├── ppg.rs
│       ├── mcp.rs
│       ├── skills.rs
│       └── system.rs          # /health, /api/system/agents, /api/llm/health
└── proto/                     # 可选：序列化结构（serde 统一）
```

---

## 3. REST 接口契约（必须对齐 backend）

> 路径前缀：`/api/*`。外部经 `deploy/nginx` 反代（`/api` → server:8080）。
> 下列所有响应需对齐 `StateResp`（`status/round/evidences/question/hypotheses/messages/report/task_id`）。

### 3.1 健康与系统
```rust
GET  /api/health                       -> 200 {"ok": true}
GET  /api/llm/health                   -> 200 {"ok": true, "models": [...]}
                                          （探测隧道 /t/<name>/v1/models；不可达则 ok=false）
GET  /api/system/agents                -> Vec<AgentRoute>   // capability/impl/model/available_impls
```

### 3.2 问诊会话生命周期
```rust
POST /api/consultations                -> StateResp        // body: CreateConsultationReq
POST /api/consultations/{cid}/images   -> {"id","url"}      // multipart: type + file
POST /api/consultations/{cid}/start    -> StateResp        // query: ?sync=false
POST /api/consultations/{cid}/answer   -> StateResp        // body: AnswerReq；?sync=false
GET  /api/consultations/{cid}          -> StateResp
GET  /api/consultations/{cid}/report   -> Report           // 404 if not ready
GET  /api/consultations/{cid}/stream   -> {task, error, segs: [StreamSeg]}  // query: ?after=0
GET  /api/consultations/{cid}/trace    -> Vec<trace_entry>
```

### 3.3 诊疗方案 / 随访 / 复诊
```rust
GET  /api/consultations/{cid}/care             -> Vec<CareTodo>
POST /api/consultations/{cid}/care/check       -> CareTodo        // body: {todo_id}
GET  /api/consultations/{cid}/followups         -> Vec<FollowUp>
POST /api/consultations/{cid}/followup/{fid}/feedback -> {ok, followup}  // body: {feedback}
POST /api/consultations/{cid}/revisit           -> RevisitImage    // body: RevisitReq
GET  /api/consultations/{cid}/revisit/compare   -> {has_baseline, changes:[...]}
POST /api/consultations/{cid}/lab               -> {...}           // body: {text}
POST /api/consultations/{cid}/ppg               -> StateResp        // body: PpgReq
```

### 3.4 家庭 / 成员（一人管理全家档案）
```rust
POST /api/families                            -> Family          // body: FamilyCreateReq
GET  /api/families                            -> Vec<Family>
GET  /api/families/{fid}                      -> Family
POST /api/families/{fid}/members              -> Member          // body: MemberAddReq
PATCH /api/families/{fid}/members/{mid}       -> Member          // body: MemberAddReq
GET  /api/families/{fid}/consultations        -> Vec<{id,status,...}>  // query: ?member_id=
```

### 3.5 Skills（热装载）
```rust
GET  /api/skills        -> {skills_dir, skills:[...], tools:[...]}
POST /api/skills/load   -> manifest   // body: {name?|path?}
POST /api/skills/unload -> {ok, unloaded}
```

### 3.6 MCP（Server 状态 / Client 连接池）
```rust
GET  /api/mcp/status    -> {server:{...}, clients:[...], capabilities:[...]}
GET  /api/mcp/tools     -> {tools:[...]}
POST /api/mcp/clients   -> {ok, name, tools}   // body: McpConnectReq
DELETE /api/mcp/clients/{name} -> {ok, disconnected}
```

### 3.7 中继/隧道（原 rrserver，保留）
```rust
GET  /healthz
POST /api/register      -> {ws_url, tunnel}    // body: {name, token}
GET  /ws/{name}         -> WebSocket 控制连接
GET  /t/{name}/*        -> 经隧道反代到家庭端本地服务（已支持 SSE 流式）
```

> **契约对齐原则**：Rust 版字段名/类型需与 `backend/app/models/schemas.py` 及 `main.py` 完全一致（见 §4），否则前端/小程序解析失败。

---

## 4. 领域数据模型（Rust 等价定义，serde）

> 下列为**类型签名草案**，字段对齐 `schemas.py`。`#[serde(default)]` 缺省值需与 Python 一致。

```rust
// models/archive.rs
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Patient {
    #[serde(default)] pub region: String,
    #[serde(default)] pub height_cm: f64,
    #[serde(default)] pub weight_kg: f64,
    #[serde(default)] pub age: i64,
    #[serde(default = "default_gender")] pub gender: String, // 男|女|未知
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Member {
    #[serde(default = "uid_m_")] pub id: String,
    pub family_id: String,
    #[serde(default)] pub name: String,
    #[serde(default = "default_relation")] pub relation: String, // 本人/父亲/...
    #[serde(default)] pub patient: Patient,
    #[serde(default)] pub note: String,
    #[serde(default = "now_f")] pub created_at: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Family {
    #[serde(default = "uid_f_")] pub id: String,
    #[serde(default = "default_family_name")] pub name: String,
    #[serde(default)] pub owner: String,
    #[serde(default)] pub members: Vec<Member>,
    #[serde(default = "now_f")] pub created_at: f64,
}

// models/consultation.rs
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageItem {
    #[serde(default = "uid_img_")] pub id: String,
    pub type_: ImageType, // tongue|face|lesion|palm_left|palm_right
    #[serde(default)] pub url: String,
    #[serde(default)] pub path: String,
    pub analysis: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Evidence {
    #[serde(default = "uid_ev_")] pub id: String,
    pub key: String,
    pub value: String,
    #[serde(default = "default_source")] pub source: Source, // 望闻问切自述检
    #[serde(default = "default_conf")] pub confidence: f64,  // 0.8
    #[serde(default)] pub round: i64,
    #[serde(default)] pub desc: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hypothesis {
    pub name: String,
    #[serde(default)] pub confidence: f64,
    #[serde(default)] pub supporting: Vec<String>,
    #[serde(default)] pub contradicting: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Consultation {
    #[serde(default = "uid_c_")] pub id: String,
    #[serde(default = "now_f")] pub ts: f64,
    #[serde(default)] pub family_id: String,
    #[serde(default)] pub member_id: String,
    pub patient: Patient,
    #[serde(default)] pub complaint: String,
    #[serde(default)] pub self_report: serde_json::Value,
    #[serde(default)] pub images: Vec<ImageItem>,
    pub ppg: Option<PpgReading>,
    pub status: ConsultStatus, // created|running|waiting_answer|planning|treatment_qa|finished|referred
    #[serde(default)] pub round: i64,
    #[serde(default)] pub evidences: Vec<Evidence>,
    #[serde(default)] pub hypotheses: Vec<Hypothesis>,
    pub current_question: Option<Question>,
    #[serde(default)] pub asked_keys: Vec<String>,
    #[serde(default)] pub treatment_answers: Vec<serde_json::Value>,
    pub report: Option<Report>,
    #[serde(default)] pub messages: Vec<Message>,
    #[serde(default)] pub trace: Vec<serde_json::Value>,
    pub task_id: Option<String>,
    #[serde(default)] pub meta: serde_json::Value,
    #[serde(default)] pub care_todos: Vec<CareTodo>,
    #[serde(default)] pub followups: Vec<FollowUp>,
    #[serde(default)] pub revisits: Vec<RevisitImage>,
    #[serde(default)] pub lab_reports: Vec<String>,
    #[serde(default)] pub stream: Vec<StreamSeg>,
    #[serde(default)] pub stream_seq: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StateResp {
    pub id: String,
    pub status: String,
    pub round: i64,
    #[serde(default)] pub family_id: String,
    #[serde(default)] pub member_id: String,
    pub ppg: Option<PpgReading>,
    #[serde(default)] pub evidences: Vec<Evidence>,
    pub question: Option<Question>,
    #[serde(default)] pub hypotheses: Vec<Hypothesis>,
    #[serde(default)] pub messages: Vec<Message>,
    pub report: Option<Report>,
    pub task_id: Option<String>,
}

// models/report.rs
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    #[serde(default)] pub syndromes: Vec<Hypothesis>,
    #[serde(default)] pub reasoning: String,
    #[serde(default)] pub advice: serde_json::Value, // {饮食,起居,建议就诊科室}
    #[serde(default)] pub treatments: Vec<TreatmentPlan>,
    pub red_flag: Option<String>,
    #[serde(default)] pub sources: Vec<String>,
    #[serde(default)] pub evolution: String,
    #[serde(default = "default_disclaimer")] pub disclaimer: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TreatmentPlan {
    #[serde(default = "uid_tp_")] pub id: String,
    pub category: TreatmentCategory, // 中药方剂|针灸推拿|外治法|西医检查|生活调护|膳食
    #[serde(default)] pub title: String,
    #[serde(default)] pub detail: String,
    #[serde(default)] pub rationale: String,
    #[serde(default)] pub note: String,
    #[serde(default)] pub warnings: Vec<String>,
    #[serde(default = "default_priority")] pub priority: i64, // 1
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CareTodo {
    #[serde(default = "uid_ct_")] pub id: String,
    #[serde(default)] pub title: String,
    #[serde(default)] pub category: String,
    #[serde(default)] pub detail: String,
    pub kind: CareKind, // decoct|checkin|appointment
    #[serde(default)] pub times: Vec<String>,
    #[serde(default)] pub done: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FollowUp {
    #[serde(default = "uid_fu_")] pub id: String,
    pub due_in_days: i64,
    #[serde(default)] pub focus: String,
    #[serde(default)] pub done: bool,
    #[serde(default)] pub feedback: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RevisitImage {
    #[serde(default = "uid_rv_")] pub id: String,
    #[serde(default = "now_f")] pub ts: f64,
    #[serde(default)] pub path: String,
    #[serde(default = "default_tongue")] pub kind: String,
    #[serde(default)] pub features: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PpgReading {
    #[serde(default)] pub rate_bpm: f64,
    #[serde(default = "default_rhythm")] pub rhythm: String, // 整齐
    #[serde(default = "default_depth")] pub depth: String,  // 中
    #[serde(default = "default_force")] pub force: String,  // 有力
    #[serde(default = "default_shape")] pub shape: String,   // 平
    #[serde(default)] pub amplitude: f64,
    #[serde(default)] pub perfusion: f64,
    #[serde(default)] pub signal_quality: f64,
    #[serde(default)] pub notes: String,
    #[serde(default = "now_f")] pub ts: f64,
}

// 流式分段
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamSeg {
    pub seq: i64,
    #[serde(default = "default_role")] pub role: String,    // agent|user|system
    #[serde(default = "default_msg_type")] pub type_: String, // text|question|report|alert
    #[serde(default)] pub content: String,
    #[serde(default)] pub done: bool,
}

// API DTO
#[derive(Debug, Deserialize)]
pub struct CreateConsultationReq {
    pub patient: Patient,
    pub complaint: String,
    #[serde(default)] pub self_report: serde_json::Value,
    #[serde(default)] pub family_id: String,
    #[serde(default)] pub member_id: String,
}

#[derive(Debug, Deserialize)]
pub struct AnswerReq {
    pub question_id: String,
    #[serde(default)] pub value: String,
    #[serde(default)] pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct PpgReq {
    #[serde(default)] pub samples: Vec<f64>,
    #[serde(default = "default_fs")] pub fs: i64,        // 50
    #[serde(default)] pub simulate: bool,
    #[serde(default = "default_profile")] pub profile: String, // normal
    #[serde(default = "default_bpm")] pub rate_bpm: f64,  // 75.0
}
```

---

## 5. 模块边界与 Rust 模块签名（草案）

### 5.1 配置 `config.rs`
```rust
pub struct Settings {
    pub host: String,
    pub port: u16,            // 8080
    pub cors_origins: Vec<String>,
    pub upload_dir: PathBuf,
    pub store: StoreConfig,   // memory | redis
    pub llm: LlmConfig,       // 默认 base_url（可指向隧道 /t/<name>/v1）
    pub routing: RoutingConfig,
    pub tunnels: Vec<TunnelConfig>,  // 原 rrserver [[tunnels]]
    pub mcp: McpConfig,
    pub skills_dir: PathBuf,
}
impl Settings { pub fn from_toml(path: &str) -> Result<Self>; }
```

### 5.2 诊断编排 `diagnose/orchestrator.rs`
```rust
pub struct Orchestrator { store: Arc<dyn ConsultationStore>, llm: LlmRouter, registry: Arc<AgentRegistry>, router: Arc<RoutingTable> }

impl Orchestrator {
    pub async fn start(&self, cid: &str) -> Result<()>;        // 异步后台 Loop
    pub async fn start_sync(&self, cid: &str) -> Result<()>;   // 同步跑完
    pub async fn answer(&self, cid: &str, value: &str, text: &str, sync: bool) -> Result<()>;
    pub async fn followup_feedback(&self, cid: &str, fid: &str, feedback: &str) -> Result<()>;
    pub async fn lab_interpret(&self, cid: &str, text: &str) -> Result<serde_json::Value>;
    // 终止条件（对齐 backend/config.py）：
    //   single_conf=0.8 | dual_conf=0.7 | min_evidences=7 | max_rounds=6
}
```

### 5.3 LLM 路由 `llm/client.rs`
```rust
pub struct LlmRouter { default_base: String, text_model: String, vision_model: String, http: reqwest::Client }

impl LlmRouter {
    // 普通补全（rule-following 用 text_model）
    pub async fn chat(&self, sys: &str, user: &str, model: Option<&str>) -> Result<String>;
    // 流式补全（诊断 Loop 边说边出，SSE 透传）
    pub async fn stream(&self, sys: &str, user: &str, model: Option<&str>) -> Result<impl Stream<Item=Result<String>>>;
    // 多模态（望诊 vision）：images 为本地路径，读取后作为 content 传入
    pub async fn chat_vision(&self, sys: &str, prompt: &str, images: &[ImageItem]) -> Result<String>;
    // 工具调用（skills/mcp）
    pub async fn tool_call(&self, sys: &str, user: &str, tools: &[ToolDef]) -> Result<ToolCallResult>;
}
```
> `LlmRouter` 的 `default_base` 指向隧道（`http://127.0.0.1:<tunnel_to_llm>/v1` 或经 `/t/<name>/v1`），从而 server 通过 llm_server 访问家庭端 LM Studio。

### 5.4 中继复用 `relay/*`
- 直接把 `rrserver/src/{server,client,llmsrv,protocol,state,skill}.rs` 迁入 `server/src/relay/`，仅在 `main.rs` 中同时挂载诊断 router 与中继 router。
- `relay::server::forward_to_tunnel` 与 `relay::client::forward_to_ws`（SSE 流式）**保持原样复用**，不重写。

### 5.5 持久化 `store/`
```rust
#[async_trait]
pub trait ConsultationStore: Send + Sync {
    fn get(&self, cid: &str) -> Option<Consultation>;
    fn save(&self, c: &Consultation);
    fn list(&self, family_id: &str, member_id: &str) -> Vec<Consultation>;
}
#[async_trait]
pub trait FamilyStore { /* get/save/list 同构 */ }
pub struct MemoryStore { map: Arc<RwLock<HashMap<String, Consultation>>> }
// redis 实现置于 feature "redis"
```

---

## 6. 7 个 Sub-Agent 映射（移植 backend/app/agents）

| Rust 模块 | Python 原型 | impl | 输入→输出 |
|---|---|---|---|
| `agents::inspection` | `agents/inspection.py` | vision(default) | 图片 → 舌/面特征证据 |
| `agents::auscultation` | `agents/auscultation.py` | rule | 文本 → 声音/气味证据 |
| `agents::interrogation` | `agents/interrogation.py` | llm | 病史 → 问诊问题(候选特征键) |
| `agents::palpation` | `agents/palpation.py` | llm+rule | ppg 证据 → 切诊结论 |
| `agents::differentiation` | `agents/differentiation.py` | llm(rule-following) | 证据池 → 证候假设 |
| `agents::knowledge` | `agents/knowledge.py` | rule | 证候 → 方剂/建议检索 |
| `agents::safety` | `agents/safety.py` | rule | 方剂 → 十八反/十九畏/孕忌校验 |

> 知识库（`knowledge/*`）与 Python 版**逐字移植**为 Rust 常量/查表（证候库、问卷、建议、用药禁忌）。

---

## 7. 诊断 Loop（对齐 orchestrator）

```
start(c):
  c.status = running
  loop round in 1..=max_rounds:
    调用 望/闻/问/切 4 个采集 agent → 汇入 evidences
    call differentiation → hypotheses（conf）
    call knowledge + safety → treatments
    if single_conf 满足 or dual_conf 满足 or evidences>=min_evidences:
        break
    q = interrogation.ask_next(c)  // 选evidence缺口最大的特征键
    c.current_question = q; c.status = waiting_answer
    return  // 等前端 answer
answer(c, v, t):
    把答案写入 evidences（source=自述/问）
    c.round += 1; c.status = running
    goto loop 继续
after loop:
    c.report = report::build(c)  // 辨证依据链 + 方案 + 待办 + 随访 + 复诊提示
    c.status = finished
```
- **流式**：loop 每步通过 `stream.rs` 推送 `StreamSeg`（seq 自增），前端 `GET /stream?after=` 增量拉取。
- **SSE**：`start`/`answer` 走后台任务，任务状态存 `store.tasks`（对齐 `get_task`）。

---

## 8. 部署与 docker

- `server/Dockerfile`：多阶段，`cargo build --release`（在 WSL2/Ubuntu 24.04 glibc 2.39 构建，COPY 预编译二进制到 `ubuntu:24.04`；debian:bookworm glibc 2.36 会报 `GLIBC_2.39 not found`）。
- `deploy/docker-compose.yml`：把 `rrserver` 服务替换为 `server` 服务（`build: ../server`，`command: ["server","--listen","0.0.0.0:8080","--config","/etc/server.toml"]`）。
- `deploy/nginx/rrserver.conf` 的 `proxy_pass http://rrserver:8080` 改为 `proxy_pass http://server:8080`。
- 证书仍用 `deploy/certs/`。
- `llm_server` 家庭端：其 `rrclient.server_base` 指向 `https://<域名>/rr`，`name`/`token` 与 `server.toml` 的 `[[tunnels]]` 一致即可注册；server 经 `/t/<name>/v1` 调 LM Studio。

---

## 9. 分阶段实现计划（建议）

| 阶段 | 内容 | 交付物 |
|---|---|---|
| **A. 骨架** | `server/` crate + `config` + `models` + `store::memory` + 复用 `relay/*` + 诊断 router 空壳 + `/health` | 可编译、可启动、隧道仍可用 |
| **B. 接口契约** | `api/*` 全部路由（§3）+ `StateResp` 对齐 | 前端/小程序可对接（无 AI 逻辑，先回 mock） |
| **C. 知识库 + 7 agent（含 rule）** | `knowledge/*` + `agents/*` + `diagnose/orchestrator` Loop（仅 rule 路径可跑） | 纯规则诊断可跑通 |
| **D. LLM 路由 + 隧道调模型** | `llm/client` + `LlmRouter` 指向 `/t/<name>/v1` + 流式 | 真实调用家庭端 LM Studio，SSE 透传 |
| **E. 家庭/PPG/MCP/skills** | `families`、`ppg`、`mcp/*`、`skills/*` 移植 | 全功能对齐 backend |
| **F. 部署** | `Dockerfile` + `deploy` 配置切换 | 生产可编排 |

---

## 10. 风险与待确认

1. **Rust 全量重写 backend 风险高**：约 70 个 Python 文件，中医规则逻辑需逐字移植，建议严格按 §9 分阶段并以 `backend` 为比对基准做契约测试。
2. **隧道 SSE**：已确认支持，但需在新 `server` 内用同一个 `relay` 模块；若把中继与诊断放在同一 axum Router，需确保 `/t/<name>/*` 路由不与 `/api/*` 冲突（前缀隔离即可）。
3. **LLM 调用 base_url 指向隧道**：`/t/<name>/v1/chat/completions` 经 WS 隧道到达家庭端 LM Studio；需家庭端 `llm_server` 已注册且健康。server 需有"隧道不可用则 fallback/报错"策略。
4. **store 持久化**：backend 默认内存；生产如需 redis，用 feature gate 实现 `RedisStore`，接口对齐 `ConsultationStore`。
5. **routing 配置格式**：backend 用 YAML（`routing.yaml`）；Rust 建议改 TOML（`routing.toml`）或复用 YAML crate，需与前端/部署脚本约定。
6. **MCP/skills 移植成本**：backend 的 MCP Server（Streamable HTTP）+ Client 连接池 + skills 热装载是独立子系统，移植工作量大但边界清晰，建议放最后（阶段 E）。
