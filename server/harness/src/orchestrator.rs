//! 诊断流程引擎（orchestrator）
//!
//! 复刻 backend `app/core/orchestrator.py` 的诊断 Loop：
//! 按 `resources/routing.yaml` 的激活顺序，依次调用 sub-agent，
//! 四诊收集证据 -> 辨证 -> 安全门 -> 治疗，支持多轮追问直到信息充分。

use crate::agents::Registry;
use crate::config::HarnessConfig;
use crate::model::{Capability, Message};
use crate::resources::ResourceBundle;
use crate::skills::SkillRegistry;
use anyhow::Result;
use serde_json::json;

/// 单次诊断运行结果
pub struct Diagnosis {
    pub steps: Vec<(Capability, String)>,
    pub final_text: String,
}

/// 执行一次完整诊断流程。
///
/// `messages` 为截至当前的对话；`registry` 提供各 agent 实现，`skills` 提供工具。
/// 返回每一步的输出，便于前端分步展示。
pub async fn run_diagnosis(
    registry: &Registry,
    cfg: &HarnessConfig,
    res: &ResourceBundle,
    llm: &reqwest::Client,
    skills: &SkillRegistry,
    messages: &[Message],
    payload: &serde_json::Value,
) -> Result<Diagnosis> {
    // 激活顺序：路由表指定，缺省为经典四诊->辨证->安全->治疗
    let order: Vec<Capability> = if res.routing.active.is_empty() {
        vec![
            Capability::Inspection,
            Capability::Listening,
            Capability::Inquiry,
            Capability::Palpation,
            Capability::Differentiation,
            Capability::Safety,
            Capability::Treatment,
        ]
    } else {
        res.routing
            .active
            .iter()
            .filter_map(|s| Capability::from_slug(s))
            .collect()
    };

    let mut steps = Vec::new();

    for cap in order {
        if let Some(agent) = registry.get(cap) {
            let ctx = crate::agents::AgentContext {
                config: std::sync::Arc::new(cfg.clone()),
                resources: std::sync::Arc::new(res.clone()),
                llm: llm.clone(),
                skills: std::sync::Arc::new(skills.clone()),
            };
            let out = agent.run(&ctx, messages, payload).await?;
            steps.push((cap, out));
        }
    }

    let final_text = steps
        .iter()
        .map(|(c, t)| format!("## {}\n{t}", c.zh()))
        .collect::<Vec<_>>()
        .join("\n\n");

    Ok(Diagnosis { steps, final_text })
}

/// 单步调用（前端按 capability 直接请求某个 agent 时用）
pub async fn run_single(
    registry: &Registry,
    cfg: &HarnessConfig,
    res: &ResourceBundle,
    llm: &reqwest::Client,
    skills: &SkillRegistry,
    cap: Capability,
    messages: &[Message],
    payload: &serde_json::Value,
) -> Result<(Capability, String)> {
    if let Some(agent) = registry.get(cap) {
        let ctx = crate::agents::AgentContext {
            config: std::sync::Arc::new(cfg.clone()),
            resources: std::sync::Arc::new(res.clone()),
            llm: llm.clone(),
            skills: std::sync::Arc::new(skills.clone()),
        };
        let out = agent.run(&ctx, messages, payload).await?;
        Ok((cap, out))
    } else {
        anyhow::bail!("未注册的 capability: {:?}", cap)
    }
}

/// 构造标准化的 HTTP 响应 payload
pub fn diagnosis_payload(d: &Diagnosis) -> serde_json::Value {
    let steps: Vec<serde_json::Value> = d
        .steps
        .iter()
        .map(|(c, t)| json!({"capability": c, "text": t}))
        .collect();
    json!({"steps": steps, "summary": d.final_text})
}
