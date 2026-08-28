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
use crate::skills::{mcp_skill, Skill, SkillFn, SkillRegistry};
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

/// 构造调度某个 sub-agent 的技能执行器（异步，可在运行时内直接 await agent）
fn agent_skill_executor(
    cap: Capability,
    cfg: HarnessConfig,
    res: Arc<ResourceBundle>,
    llm: reqwest::Client,
) -> SkillFn {
    Arc::new(move |args: &Value| {
        let text = args.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
        let cfg = cfg.clone();
        let res = res.clone();
        let llm = llm.clone();
        Box::pin(async move {
            let msgs = vec![Message {
                role: "user".to_string(),
                content: text,
            }];
            let empty_skills = SkillRegistry::new();
            let ctx = crate::agents::AgentContext {
                config: std::sync::Arc::new(cfg),
                resources: res,
                llm,
                skills: std::sync::Arc::new(empty_skills),
            };
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
        (Capability::Inspection, "tcm-vision", "中医望诊：观察神色形态、舌象"),
        (Capability::Listening, "tcm-auscultation", "中医闻诊：听声音、嗅气味"),
        (Capability::Inquiry, "tcm-inquiry", "中医问诊：系统追问症状"),
        (Capability::Palpation, "tcm-palpation", "中医切诊：脉象与体检数据解析"),
        (Capability::Differentiation, "tcm-reference", "中医辨证参考：综合四诊给证候倾向"),
        (Capability::Safety, "tcm-safety", "安全门：红色警戒与用药安全校验"),
    ];
    for (cap, name, desc) in four {
        let exec = agent_skill_executor(cap, cfg.clone(), res.clone(), llm.clone());
        reg.register(Skill::new(name, desc, obj_param(), exec).with_owner(cap));
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
                let q = args.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string();
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
    }));

    reg.register(Skill::new(
        "tcm-diet",
        "食疗建议：按证候给食疗方",
        json!({
            "type": "object",
            "properties": {
                "syndrome": {"type": "string", "description": "证候 slug 或中文名"}
            },
            "required": ["syndrome"]
        }),
        {
            let r = res.clone();
            let exec: SkillFn = Arc::new(move |args: &Value| {
                let res = r.clone();
                let s = args.get("syndrome").and_then(|v| v.as_str()).unwrap_or("").to_string();
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
        }
    ));

    reg.register(Skill::new(
        "tcm-rag",
        "RAG 检索：从中医文献向量库取相关段落",
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "检索问题"}
            },
            "required": ["query"]
        }),
        {
            let rag = Arc::new(cfg.rag_endpoint.clone());
            let exec: SkillFn = Arc::new(move |args: &Value| {
                let rag = rag.clone();
                let q = args.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string();
                Box::pin(async move {
                    if let Some(rag) = rag.as_ref() {
                        match reqwest::Client::new()
                            .post(rag)
                            .json(&json!({"query": q}))
                            .send()
                            .await
                        {
                            Ok(r) => match r.error_for_status() {
                                Ok(resp) => match resp.json::<Value>().await {
                                    Ok(v) => Ok(v),
                                    Err(e) => Ok(json!({"error": e.to_string()})),
                                },
                                Err(e) => Ok(json!({"error": e.to_string()})),
                            },
                            Err(e) => Ok(json!({"error": e.to_string()})),
                        }
                    } else {
                        Ok(json!({"result": "RAG 未配置（设置 HARNESS_RAG_ENDPOINT）", "query": q}))
                    }
                })
            });
            exec
        }
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
