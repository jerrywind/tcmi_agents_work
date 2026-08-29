//! 内置技能注册表（复刻 backend `app/skills/registry.py`）
//!
//! 把 backend 的 9 个工具封装为本地 Skill：每个工具内部调用对应 sub-agent 或
//! 知识函数（无需远端 MCP 即可独立运行）；若配置了外部 MCP/HTTP 端点，则改用
//! `mcp_skill` / `http_skill` 转发。
//!
//! 工具清单（name -> owner capability）：
//! - tcm-vision        望诊            (owner: inspection)
//! - tcm-auscultation  闻诊            (owner: listening)
//! - tcm-inquiry       问诊            (owner: inquiry)
//! - tcm-palpation     切诊            (owner: palpation)
//! - tcm-reference     辨证参考        (owner: differentiation)
//! - tcm-safety        安全门          (owner: safety)
//! - tcm-kb            知识库检索       (全局)
//! - tcm-diet          食疗建议        (全局)
//! - tcm-rag           RAG 检索        (全局)

use crate::config::HarnessConfig;
use crate::model::{Capability, Message};
use crate::resources::ResourceBundle;
use crate::skills::{mcp_skill, mcp_skill_named, Skill, SkillFn, SkillRegistry};
use serde_json::{json, Value};
use std::sync::Arc;

fn obj_param() -> Value {
    json!({
        "type": "object",
        "properties": {
            "text": {"type": "string", "description": "患者输入文本"}
        },
        "required": ["text"]
    })
}

/// 按证候检索的入参（方剂 / 调护 / 食疗共用）
fn syndrome_param() -> Value {
    json!({
        "type": "object",
        "properties": {
            "syndrome": {"type": "string", "description": "证候 slug 或中文名"}
        },
        "required": ["syndrome"]
    })
}

/// 构造调度某个 sub-agent 的技能执行器（异步，可在运行时内直接 await agent）
fn agent_skill_executor(
    cap: Capability,
    cfg: HarnessConfig,
    res: Arc<ResourceBundle>,
    llm: reqwest::Client,
) -> SkillFn {
    Arc::new(move |args: &Value| {
        let text = args
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let cfg = cfg.clone();
        let res = res.clone();
        let llm = llm.clone();
        Box::pin(async move {
            let msgs = vec![Message {
                role: "user".to_string(),
                content: text,
            }];
            let empty_skills = SkillRegistry::new();
            let ctx = crate::agents::AgentContext::new(
                std::sync::Arc::new(cfg),
                res,
                llm,
                std::sync::Arc::new(empty_skills),
            );
            if let Some(agent) = crate::agents::Registry::new().get(cap) {
                match agent.run(&ctx, &msgs, &json!({})).await {
                    Ok(out) => Ok(json!({"result": out})),
                    Err(e) => Ok(json!({"error": e.to_string()})),
                }
            } else {
                Ok(json!({"error": "agent 未注册"}))
            }
        })
    })
}

/// 构建默认技能注册表
pub fn build_default_registry(
    cfg: &HarnessConfig,
    res: &ResourceBundle,
    llm: reqwest::Client,
) -> SkillRegistry {
    let mut reg = SkillRegistry::new();
    let res = Arc::new(res.clone());

    // 四诊 + 辨证 + 安全：各自专属
    let four = [
        (
            Capability::Inspection,
            "tcm-vision",
            "中医望诊：观察神色形态、舌象",
        ),
        (
            Capability::Listening,
            "tcm-auscultation",
            "中医闻诊：听声音、嗅气味",
        ),
        (Capability::Inquiry, "tcm-inquiry", "中医问诊：系统追问症状"),
        (
            Capability::Palpation,
            "tcm-palpation",
            "中医切诊：脉象与体检数据解析",
        ),
        (
            Capability::Differentiation,
            "tcm-reference",
            "中医辨证参考：综合四诊给证候倾向",
        ),
        (
            Capability::Safety,
            "tcm-safety",
            "安全门：红色警戒与用药安全校验",
        ),
    ];
    for (cap, name, desc) in four {
        let exec = agent_skill_executor(cap, cfg.clone(), res.clone(), llm.clone());
        reg.register(Skill::new(name, desc, obj_param(), exec).with_owner(cap));
    }

    // 治疗专属：方剂 / 调护检索（T2.3）
    //
    // 此前 treatment 只能用全局技能，方剂与调护虽已在 knowledge 层实现，
    // 却没有暴露给模型——治疗步只能拿规则拼好的文本，模型无法按需查证。
    for (name, desc, kind) in [
        (
            "tcm-formula",
            "方剂检索：按证候查适用方剂、组成、用法与禁忌",
            "formula",
        ),
        (
            "tcm-care",
            "调护方案：按证候查饮食/起居/情志调护条目",
            "care",
        ),
    ] {
        let r = res.clone();
        let kind = kind.to_string();
        let exec: SkillFn = Arc::new(move |args: &Value| {
            let res = r.clone();
            let kind = kind.clone();
            let s = args
                .get("syndrome")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Box::pin(async move {
                let slug = res
                    .syndrome(&s)
                    .map(|x| x.slug.clone())
                    .unwrap_or_else(|| s.clone());
                let result = if kind == "formula" {
                    crate::knowledge::find_formula(&res, &slug)
                } else {
                    crate::knowledge::find_care(&res, &slug)
                };
                Ok(json!({"result": result, "syndrome": slug}))
            })
        });
        reg.register(
            Skill::new(name, desc, syndrome_param(), exec).with_owner(Capability::Treatment),
        );
    }

    // 全局工具：知识库 / 食疗 / RAG
    reg.register(Skill::new(
        "tcm-kb",
        "中医知识库检索：按关键词查证候/方剂",
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "检索词"}
            },
            "required": ["query"]
        }),
        {
            let r = res.clone();
            let exec: SkillFn = Arc::new(move |args: &Value| {
                let res = r.clone();
                let q = args
                    .get("query")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Box::pin(async move {
                    let hit = res
                        .syndromes
                        .iter()
                        .find(|s| s.name.contains(&q) || s.slug.contains(&q))
                        .map(|s| json!({"name": s.name, "pathogenesis": s.pathogenesis}))
                        .unwrap_or(json!(null));
                    Ok(json!({"result": hit}))
                })
            });
            exec
        },
    ));

    reg.register(Skill::new(
        "tcm-diet",
        "食疗建议：按证候给食疗方",
        syndrome_param(),
        {
            let r = res.clone();
            let exec: SkillFn = Arc::new(move |args: &Value| {
                let res = r.clone();
                let s = args
                    .get("syndrome")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Box::pin(async move {
                    let slug = res
                        .syndrome(&s)
                        .map(|x| x.slug.clone())
                        .unwrap_or_else(|| s.clone());
                    let cares = crate::knowledge::find_care(&res, &slug);
                    Ok(json!({"result": cares}))
                })
            });
            exec
        },
    ));

    reg.register(Skill::new(
        "tcm-rag",
        "RAG 检索：从中医文献向量库取相关段落",
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "检索问题"},
                "top_k": {
                    "type": "integer",
                    "description": "返回条数，缺省由 RAG 服务决定",
                    "minimum": 1
                }
            },
            "required": ["query"]
        }),
        {
            let rag = Arc::new(cfg.rag_endpoint.clone());
            let exec: SkillFn = Arc::new(move |args: &Value| {
                let rag = rag.clone();
                let q = args
                    .get("query")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let top_k = args.get("top_k").and_then(|v| v.as_u64()).map(|n| n as u32);
                Box::pin(async move {
                    let Some(endpoint) = rag.as_ref() else {
                        return Ok(json!({
                            "result": "RAG 未配置（设置 HARNESS_RAG_ENDPOINT）",
                            "query": q
                        }));
                    };

                    // 与 llm_server/rag 的契约对齐：
                    //   POST <endpoint>  {"query": "...", "top_k"?: N}
                    // endpoint 需指向具体检索端点，例如
                    //   http://<rag-host>:8080/rag/retrieve/text
                    // 该端点返回**数组**，这里统一包成 {"result": [...]}，
                    // 使其余技能的返回形状保持一致。
                    let mut body = json!({"query": q});
                    if let Some(k) = top_k {
                        body["top_k"] = json!(k);
                    }

                    let resp = match reqwest::Client::new()
                        .post(endpoint)
                        .json(&body)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return Ok(json!({"error": e.to_string()})),
                    };
                    let resp = match resp.error_for_status() {
                        Ok(r) => r,
                        Err(e) => return Ok(json!({"error": e.to_string()})),
                    };
                    match resp.json::<Value>().await {
                        // rag 服务返回数组 → 包一层；已是对象则原样透传
                        Ok(v) if v.is_array() => Ok(json!({"result": v})),
                        Ok(v) => Ok(v),
                        Err(e) => Ok(json!({"error": e.to_string()})),
                    }
                })
            });
            exec
        },
    ));

    reg
}

/// 按配置挂载外部 MCP 工具（可选）
pub fn mount_mcp(
    reg: &mut SkillRegistry,
    name: &str,
    description: &str,
    mcp_url: &str,
    llm: reqwest::Client,
) {
    let skill = mcp_skill(name, description, obj_param(), mcp_url.to_string(), llm);
    reg.register(skill);
}

/// 从 `tools/list` 的响应里解析工具清单
///
/// MCP 的返回是 `{"tools": [{"name", "description"?, "inputSchema"?}]}`；
/// 各 server 实现质量不一，缺字段的按缺省值兜底而不是丢弃整条。
fn parse_mcp_tools(v: &Value) -> Vec<(String, String, Value)> {
    v.get("tools")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let name = t.get("name").and_then(|n| n.as_str())?;
                    let desc = t
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string();
                    let schema = t
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
                    Some((name.to_string(), desc, schema))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 按 `config.yaml` 的 `mcp_clients` 挂载全部外部 MCP server（T2.4）
///
/// 启动时对每个 server 发一次 `tools/list`，把工具挂成名为
/// `mcp__<client>__<tool>` 的全局技能，随后 `GET /skills` 可见、
/// 各 agent 的 tool calling 可直接调用。
///
/// **单个 server 不可用只告警不启动失败**：MCP 多为外部依赖，
/// 让整个 harness 起不来会放大故障面。
pub async fn mount_mcp_clients(
    reg: &mut SkillRegistry,
    cfg: &HarnessConfig,
    client: &reqwest::Client,
) {
    for c in &cfg.mcp_clients {
        if !c.enabled {
            tracing::info!(mcp = %c.name, "MCP client 已停用，跳过挂载");
            continue;
        }
        match crate::mcp::list_tools(client, &c.url).await {
            Ok(listed) => {
                let tools = parse_mcp_tools(&listed);
                if tools.is_empty() {
                    tracing::warn!(mcp = %c.name, url = %c.url, "MCP server 未返回任何工具");
                    continue;
                }
                let mut mounted = 0usize;
                for (tool, desc, schema) in tools {
                    // 白名单：配置留空表示挂载该 server 的全部工具
                    if !c.tools.is_empty() && !c.tools.iter().any(|t| t == &tool) {
                        continue;
                    }
                    let name = format!("mcp__{}__{}", c.name, tool);
                    let description = if desc.is_empty() {
                        format!("[MCP:{}] {}", c.name, tool)
                    } else {
                        format!("[MCP:{}] {}", c.name, desc)
                    };
                    reg.register(mcp_skill_named(
                        &name,
                        &tool,
                        &description,
                        schema,
                        c.url.clone(),
                        client.clone(),
                    ));
                    mounted += 1;
                }
                tracing::info!(mcp = %c.name, url = %c.url, mounted, "已挂载 MCP 工具");
            }
            Err(e) => {
                tracing::warn!(mcp = %c.name, url = %c.url, error = %e, "MCP server 不可达，跳过挂载");
            }
        }
    }
}
