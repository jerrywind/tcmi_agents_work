# Sub-Agent 协议规范（harness）

定义 harness 中 7 个 Sub-Agent 的**标识、接口契约、注册方式与编排规则**。
实现见 `server/harness/src/agents/`、`src/orchestrator.rs`、`src/model.rs`。

---

## 1. 能力标识（Capability）

`src/model.rs` 中的 `Capability` 枚举，`serde(rename_all = "snake_case")`，
**序列化为无前缀 slug**，用于 `resources/routing.yaml`、`POST /agents` 的 `capability` 字段。

| slug | 枚举变体 | 子项 | `zh()` |
|---|---|---|---|
| `inspection` | `Inspection` | 望诊 | 望诊 |
| `listening` | `Listening` | 闻诊 | 闻诊 |
| `inquiry` | `Inquiry` | 问诊 | 问诊 |
| `palpation` | `Palpation` | 切诊 | 切诊 |
| `differentiation` | `Differentiation` | 辨证 | 辨证 |
| `safety` | `Safety` | 安全门 | 安全门 |
| `treatment` | `Treatment` | 治疗 | 治疗 |

解析：`Capability::from_slug(s) -> Option<Self>`，未知 slug 返回 `None`。
编排器对 `routing.yaml` 中无法解析的条目**静默跳过**（`filter_map`）。

> 历史文档里出现过带命名空间的 `diagnosis.inspection` / `treatment.plan` 写法，
> 那是原 Python backend 的协议层命名，**harness 不使用**，请勿按它拼 slug。

---

## 2. 接口契约

```rust
#[async_trait]
pub trait SubAgent: Send + Sync {
    fn capability(&self) -> Capability;
    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],   // 完整对话历史（harness 无会话状态）
        payload: &Value,        // 任意附加数据（性别/年龄/地区/体检值等）
    ) -> Result<String>;        // 该步的自然语言输出

    /// 可选的结构化输出（T4.1）：随响应原样返回给调用方，默认 `None`
    fn structured(&self, ctx: &AgentContext, messages: &[Message]) -> Option<Value>;
}

pub struct AgentContext {
    pub config: Arc<HarnessConfig>,
    pub resources: Arc<ResourceBundle>,   // 已加载的 YAML 资源快照
    pub llm: reqwest::Client,
    pub skills: Arc<SkillRegistry>,       // 该 agent 可见的技能
}
```

关键约定：

- **无状态**：Agent 不持有会话状态，全部输入经参数传入；返回值为该步输出文本。
- **单步失败不中断整体**（2026-08-29 修复）：编排器会记录失败步骤并继续跑完剩余步骤，
  `/chat` 返回 `steps` + `failures` + `partial: true`；只有**全部步骤都失败**
  （通常是 LLM 不可达）才返回 `{"error"}`。旧行为是用 `?` 直接向上传播，
  导致前几步已经付出的 LLM 开销全部作废。
- **统一出参类型**：`run` 返回 `String`（不是结构化信封），是给人看的 Markdown。
- **结构化输出另开一条通道**（T4.1）：`run` 的文本只能整段渲染，而有些结论
  （证候 / 置信度 / 证据链）要被**程序**消费——卡片化展示、兼证并列、后续检索都
  需要具体字段，从 Markdown 反解析既脆又易碎。故 trait 提供 `structured()`：
  - 默认返回 `None`，只有需要的 agent 覆写（当前仅 `DifferentiationAgent`）；
  - 编排器在每步成功后调用它，结果随 `/chat` 的 `structured.<capability>`
    与 `POST /agents` 的 `structured` 返回；
  - **必须是确定性结果**（不依赖 LLM 输出）：否则无法写回归测试，
    也会让同一份输入产出不同结构。
- **周期**：每次请求 `Registry::new()` 新建一个实例（无跨请求缓存）。

### 2.1 请求/响应模型（`src/model.rs`）

当前只保留实际被使用的三个类型：

| 类型 | 用途 |
|---|---|
| `Message { role, content }` | 对话消息（`role: user\|assistant\|system`） |
| `Capability` | 能力枚举（7 个 slug） |
| `AgentRequest { capability, messages, payload }` | `POST /agents` 的请求体 |

> 2026-08-29 清理：原 `AgentResponse` / `SkillCall` / `SkillResult` /
> `RequestFrame` / `ResponseFrame` 五个类型是从 Python schema 平移过来的残留，
> 在 Rust 侧无任何引用点（`SubAgent::run` 直接返回 `String`，HTTP 层用
> `serde_json::Value`，rrserver 用自己的 `protocol.rs`），已删除。

---

## 3. 注册与编排

```rust
// src/agents/mod.rs
impl Registry {
    pub fn new() -> Self {
        let mut map = HashMap::new();
        map.insert(Capability::Inspection, Arc::new(inspection::InspectionAgent));
        // ... 其余 6 个
        Self { map }
    }
}
```

- **编译期硬编码注册**：7 个 Agent 与 capability 一一对应，**没有 `@register` 装饰器，
  没有 `impl` 切换，没有 `routing.yaml` 的实现路由**。`routing.yaml` 只决定「激活哪些步骤及顺序」。
- **编排**：`orchestrator::run_diagnosis` 按 `routing.yaml` 的 `active` 顺序**串行**执行，
  未配置时回退为经典顺序（望→闻→问→切→辨证→安全门→治疗）。
  每一步的输出收集为 `steps`，最终拼成 Markdown `summary`。
- **无并发、无灰度、无 AB 分流**。

### 3.1 调整流程顺序

编辑 `server/harness/resources/routing.yaml` 的 `active` 列表即可增删步骤，
改完 `POST /reload`（需 `config.yaml` 中 `hot_reload: true`）或重启：

```yaml
active:
  - inspection       # 望诊
  - listening        # 闻诊
  - inquiry          # 问诊
  - palpation        # 切诊
  - differentiation  # 辨证
  - safety           # 安全门
  - treatment        # 治疗
default: inspection
```

> `default` 字段目前**未被代码读取**（`run_diagnosis` 直接按 `active` 顺序从头执行）。

---

## 4. 模型抽象

所有 Agent 共用 `config.llm_base_url`（OpenAI 兼容 `/chat/completions`），
固定参数 `temperature: 0.3`、`stream: false`。

**没有逻辑模型名映射**（原 backend 的 `llm.models` 逻辑名 → 实际模型 id 机制未移植）：
全系统只有一个 `config.model`。**没有 rule/mock 实现，也没有降级**：
LLM 不可用时 `run` 返回 `Err`，`/chat` 报错。

---

## 5. 扩展方式

### 5.1 新增一个 Sub-Agent

1. `src/model.rs` 的 `Capability` 增加变体，并在 `zh()` / `slug()` 补分支
   （`from_slug()` 由 `slug()` 派生，无需另写一份映射）；
2. 新建 `src/agents/xxx.rs`，实现 `SubAgent` trait；
   若该步有需要被程序消费的结论，一并覆写 `structured()`（见 2. 关键约定）；
3. `src/agents/mod.rs` 中 `pub mod xxx;` 并在 `Registry::new()` 插入；
4. `resources/routing.yaml` 的 `active` 加入该 slug；
5. 在 Docker 内 `cargo build -p harness` 后重启（后端一律走 Docker）。

### 5.2 远程 Sub-Agent（未实现）

协议本身是无状态的，理论上可实现一个 `HttpProxyAgent` 把 `run` 转发到远端。
当前**没有该实现**，也没有配置项。替代方案：
- 用 `POST /agents` 由调用方自行远程调用单个能力；
- 或经 MCP 接入：harness 已内置 MCP Server（`POST /mcp`），
  外部客户端可调用 7 个 `agent_*` 工具，见 [`mcp.md`](./mcp.md)。

---

## 6. 可观测性

**已有逐步埋点（T3.1）**：每个 Sub-Agent 步骤结束后，编排器把累加器快照成
`StepTrace`，随 `/chat` 的 `trace[]` 与 `POST /agents` 的 `trace` 返回，并写 tracing 日志：

| 字段 | 含义 |
|---|---|
| `capability` / `name` | 步骤 slug 与中文名 |
| `duration_ms` | 步骤总耗时（含规则计算与 LLM 等待） |
| `model` | 使用的模型 |
| `llm_calls` / `llm_attempts` | LLM 调用次数 / 含重试的请求次数 |
| `llm_duration_ms` | LLM 请求累计耗时 |
| `prompt_tokens` / `completion_tokens` / `total_tokens` | token 用量（多轮工具调用下按轮求和） |
| `tool_calls[]` | 实际调用过的工具名（按调用顺序） |
| `error` | 失败原因 |

失败步骤**同样产出一条**埋点（带 `error`），因此「某一步慢/贵/失败」可直接观测，
不必重放整次问诊。`tracing` 默认 `info,harness=debug`，可用 `RUST_LOG` 调整。

**尚未具备**：跨请求的调用链聚合（如按会话汇总）、指标导出（Prometheus）。
