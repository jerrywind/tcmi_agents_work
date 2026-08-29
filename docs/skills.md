# SKILL 工具集（LLM 可调用技能）

> SKILL = 一个 **LLM 可调用的工具（function calling）**。harness 启动时把 11 个内置技能
> 注册进全局 `SkillRegistry`，再按 `config.yaml` 的 `mcp_clients` 挂载外部 MCP 工具；
> 各 Sub-Agent 在推理时只能看到「自己专属的 + 全局的」工具。

- 实现：`server/harness/src/skills/`（`mod.rs` 注册表 / `toolcall.rs` 技能定义与分发 / `builtin.rs` 内置技能）
- 消费方：`server/harness/src/agents/base.rs::chat_with_tools`
- 查询：`GET /skills`；执行：`POST /skills`

---

## 1. 数据模型

```rust
pub struct Skill {
    pub name: String,             // 工具名（LLM 看到的 function name）
    pub description: String,      // 工具描述（决定是否被调用，写清楚"何时调用"）
    pub parameters: Value,        // JSON Schema 形式的入参定义
    pub owner: Option<Capability>,// Some(cap) = 仅该 Sub-Agent 可见；None = 全局可见
    pub executor: SkillFn,        // 异步执行体
}
pub type SkillFn = Arc<dyn Fn(&Value) -> BoxFuture<'static, Result<Value>> + Send + Sync>;
```

注册表 `SkillRegistry::for_capability(cap)` 的可见性规则：
**`owner.is_none()`（全局）或 `owner == Some(cap)`（专属）**。
因此每个 Sub-Agent 实际可见的是「专属技能（treatment 为 2 个，其余各 1 个）+ 3 个全局技能
+ 全部 `mcp__*` 外部工具」。

---

## 2. 内置技能清单（11 个）

| 技能 | owner | 入参 | 行为 |
|---|---|---|---|
| `tcm-vision` | `inspection` | `{"text": string}` | 以 `text` 为输入**重跑望诊 Agent**，返回其输出 |
| `tcm-auscultation` | `listening` | `{"text": string}` | 重跑闻诊 Agent |
| `tcm-inquiry` | `inquiry` | `{"text": string}` | 重跑问诊 Agent |
| `tcm-palpation` | `palpation` | `{"text": string}` | 重跑切诊 Agent |
| `tcm-reference` | `differentiation` | `{"text": string}` | 重跑辨证 Agent |
| `tcm-safety` | `safety` | `{"text": string}` | 重跑安全门 Agent |
| `tcm-kb` | 全局 | `{"query": string}` | 在 `syndromes.yaml` 中按 slug/中文名子串匹配，返回 `{name, pathogenesis}`（未命中为 `null`） |
| `tcm-diet` | 全局 | `{"syndrome": string}` | 按证候 slug 或中文名解析出 slug，返回 `care.yaml` 的调护条目 |
| `tcm-rag` | 全局 | `{"query": string, "top_k"?: number}` | `POST` `{"query": ..., "top_k"?: N}` 到 `HARNESS_RAG_ENDPOINT`；未配置时返回提示串而非报错 |
| `tcm-formula` | `treatment` | `{"syndrome": string}` | 按证候 slug 或中文名查 `formulas.yaml`，返回方剂的名称/组成/用法/禁忌 |
| `tcm-care` | `treatment` | `{"syndrome": string}` | 按证候查 `care.yaml` 的调护条目（饮食/起居/情志） |

> **专属技能的实现细节**：6 个专属技能由 `agent_skill_executor` 统一构造，内部用**空的
> `SkillRegistry`** 重跑对应 Agent（`builtin.rs` 第 52 行），因此技能调用 Agent、Agent 再调技能
> **不会递归**。

### 2.1 各 Capability 可见的工具

| capability | 可见工具 |
|---|---|
| `inspection` | `tcm-vision` + `tcm-kb`、`tcm-diet`、`tcm-rag` |
| `listening` | `tcm-auscultation` + 3 个全局 |
| `inquiry` | `tcm-inquiry` + 3 个全局 |
| `palpation` | `tcm-palpation` + 3 个全局 |
| `differentiation` | `tcm-reference` + 3 个全局 |
| `safety` | `tcm-safety` + 3 个全局 |
| `treatment` | `tcm-formula`、`tcm-care` + 3 个全局 |

> 若 `config.yaml` 配了 `mcp_clients`，上表每个 capability 还会额外看到全部 `mcp__*` 工具。

---

## 3. LLM 如何调用技能

7 个 Sub-Agent **全部**通过 `LlmCaller::chat_with_tools` 调用模型
（`ctx.caller()` 取得，2026-08-29 已接线；此前它们都调用无工具版本的 `chat_completion`，
导致技能在推理中完全不起作用）。

`chat_with_tools`（`agents/base.rs`）的流程：

1. 取 `skills.for_capability(cap)`；**为空则退化为普通 `chat`**（无工具调用）。
2. 把工具声明作为 `tools` 发给 LLM，`tool_choice: "auto"`，`temperature: 0.3`，`stream: false`。
3. 若返回 `tool_calls`：**一次性执行全部调用**（`dispatch` 逐个执行，异常转为 `{"error": ...}`），
   以 `role: "tool"` 回填对话，然后把**带工具**的请求再发一轮。
4. 循环第 2~3 步，最多 `max_tool_rounds` 轮（默认 3，见 `config.yaml`）。
5. 达到轮数上限、或模型不再请求工具时，发一次**不带 tools 的汇总调用**拿到最终文本。

> 即多轮 ReAct 循环（T2.2 已实现）：模型可在工具结果之上继续查证。
> `max_tool_rounds: 1` 即退化为此前的「1 次带工具 + 1 次汇总」行为。
>
> 每轮 LLM 请求都走同一套**重试**（超时 / 连接失败 / 5xx / 429，次数与退避见
> `llm_max_retries` / `llm_retry_backoff_ms`），并把耗时、token、工具名写入步骤埋点。

---

## 4. REST 端点

```bash
# 列出（返回技能的 name / description / owner；owner 为空时展示为"全局"）
curl http://localhost:8011/skills

# 只看某个 capability 用得到的工具（专属 + 全局 + mcp__*）
curl 'http://localhost:8011/skills?owner=treatment'     # 也可用中文名 owner=治疗

# 执行（arguments 见上表）
curl -X POST http://localhost:8011/skills \
  -H 'Content-Type: application/json' \
  -d '{"name":"tcm-kb","arguments":{"query":"脾胃湿热"}}'
# -> {"result":{"name":"脾胃湿热","pathogenesis":"..."}}

curl -X POST http://localhost:8011/skills \
  -H 'Content-Type: application/json' \
  -d '{"name":"tcm-diet","arguments":{"syndrome":"脾胃湿热"}}'
```

**owner 过滤**：`POST /skills` 与 `GET /skills` 都接受可选 `owner`（slug 或中文名）。
带上即把可调用范围限制为「该 capability 的专属技能 + 全局技能」，越界调用返回
`{"error":"未知技能: xxx"}`；不带则不过滤（保持旧行为）。这保证 REST 侧与 LLM 侧
（`for_capability`）看到的是同一套权限。

错误统一为 `{"error": "..."}`（**HTTP 状态码仍是 200**，调用方须检查 `error` 字段）。

---

## 5. 扩展方式

### 5.1 补充中医知识（首选，无需写代码）

方剂、食疗、调护、证候、红旗规则全部在 `server/harness/resources/*.yaml`，改完
`POST /reload`（需 `config.yaml` 中 `hot_reload: true`）或重启即可。
详见根 [`README.md`](../README.md)「流程与数据分离」。

### 5.2 新增一个技能（需改 Rust + 重新构建）

在 `src/skills/builtin.rs::build_default_registry` 中追加：

```rust
reg.register(
    Skill::new(
        "my-skill",
        "一句话说清这个技能做什么、何时该调用它",
        json!({
            "type": "object",
            "properties": { "q": {"type":"string","description":"查询参数"} },
            "required": ["q"]
        }),
        Arc::new(move |args: &Value| { /* ... */ Box::pin(async move { Ok(json!(...)) }) }),
    )
    .with_owner(Capability::Inquiry),   // 省略 with_owner 则为全局可见
);
```

然后 `cargo build -p harness` 并重启，`GET /skills` 可见。

### 5.3 挂载外部 MCP 工具（已接线）

`config.yaml` 声明即可，无需改代码：

```yaml
mcp_clients:
  - name: kb                            # 工具以 mcp__kb__<tool> 注册
    url: "http://127.0.0.1:9000/mcp"
    enabled: true                       # false 可临时停用而不删配置
    tools: []                           # 白名单，留空 = 挂载全部
```

启动时对每个 server 发一次 `tools/list`，把工具逐个注册为**全局**技能
（`owner` 为空，所有 capability 可见）；server 不可达只告警、不阻断启动。

| 构造器 | 作用 | 状态 |
|---|---|---|
| `mount_mcp_clients(&mut reg, cfg, client)` | 按配置批量挂载（**启动时的调用方**） | 已接线 |
| `mcp_skill_named(name, remote_tool, desc, params, url, client)` | 显示名与远端工具名不同时使用 | 已接线 |
| `mcp_skill(name, desc, params, mcp_url, client)` | 显示名 = 远端工具名 | 已实现，供手工注册 |
| `http_skill(name, desc, params, endpoint, client)` | 把调用转发到外部 HTTP 端点 | 已实现，供手工注册 |

详见 [`mcp.md`](./mcp.md)。

---

## 6. 已知缺口

1. **无热装载**：内置技能在编译期注册、MCP 工具在启动时挂载，运行时不能增删
   （`POST /reload` 只重载 YAML 资源，不重建技能注册表）。
2. **`GET /skills`、`GET /agents` 的顺序是刻意稳定的**：两者内部都用 `HashMap` 存储，
   直接遍历会得到**每次进程启动都可能不同**的顺序（Rust 的 HashMap 用随机化哈希）。
   `SkillRegistry::all()` 已按名称排序、`Registry::capabilities()` 已按
   望→闻→问→切→辨证→安全门→治疗 的规范顺序输出，新增遍历时请勿绕过。

---

## 7. 测试

- `tests/behavior.rs`：技能归属（`treatment` 专属方剂/调护工具、专属技能不泄漏到其他
  capability）、`Capability::from_name` 中英文解析、`mcp_clients` 配置解析、埋点累加。
- `cd server && cargo test -p harness`：技能注册、`for_capability` 的 owner 过滤、同步执行与错误分支。
- 案例回归 `cargo test -p harness --test cases` 会校验 `tcm-kb` / `tcm-diet` 所依赖的
  `syndromes.yaml` / `care.yaml` 数据完整性（每个基准证候必须有方剂或调护数据）。
- 手工验证：`GET /skills` + `POST /skills`（见第 4 节）。
