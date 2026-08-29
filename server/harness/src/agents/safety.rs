//! 安全门 sub-agent
//!
//! 扫描红色警戒关键词（resources/safety.yaml），并校验处方用药安全
//! （knowledge::herb_safety），给出转诊/警示建议。
//!
//! 命中 **high/critical** 级别时给出结构化 `blocked` 回执（T3.3）：
//! `orchestrator` 依据同一套判定中断后续步骤，不再输出治疗建议。

use crate::agents::base::{AgentContext, SubAgent};
use crate::agents::{detect_red_flags, is_blocking};
use crate::model::{Capability, Message};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct SafetyAgent;

#[async_trait]
impl SubAgent for SafetyAgent {
    fn capability(&self) -> Capability {
        Capability::Safety
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],
        payload: &Value,
    ) -> Result<String> {
        let text: String = messages
            .iter()
            .map(|m| m.content.clone())
            .collect::<Vec<_>>()
            .join("\n");

        // 1) 红色警戒（与 orchestrator 的中断判定共用同一函数）
        let hits = detect_red_flags(&ctx.resources, &text);
        let alerts: Vec<String> = hits
            .iter()
            .map(|rf| format!("[{}] {}", rf.severity, rf.advice))
            .collect();
        let blocked = hits.iter().find(|rf| is_blocking(rf));

        // 2) 用药安全（若 payload 带了处方 herbs）
        let mut herb_hits: Vec<String> = Vec::new();
        if let Some(herbs) = payload.get("herbs").and_then(|h| h.as_array()) {
            let list: Vec<String> = herbs
                .iter()
                .filter_map(|h| h.as_str().map(|s| s.to_string()))
                .collect();
            let pregnant = payload
                .get("pregnant")
                .and_then(|p| p.as_bool())
                .unwrap_or(false);
            let hits = crate::knowledge::check_herb_safety(&list, pregnant);
            herb_hits = hits.iter().map(|h| h.detail.clone()).collect();
        }

        if alerts.is_empty() && herb_hits.is_empty() {
            return Ok(format!(
                "【安全门】未触发红色警戒，处方用药无显著禁忌。\n\n{}",
                json!({"red_flags": [], "herb_safety": [], "blocked": false})
            ));
        }

        let mut out = String::from("【安全门警示】\n");
        for a in &alerts {
            out.push_str(&format!("- {a}\n"));
        }
        for h in &herb_hits {
            out.push_str(&format!("- {h}\n"));
        }

        // LLM 进一步解释（如有 system prompt）
        if !ctx.resources.prompts.safety.is_empty() {
            let llm = ctx
                .caller()
                .chat_with_tools(&ctx.resources.prompts.safety, messages, Capability::Safety)
                .await?;
            if !llm.trim().is_empty() {
                out.push('\n');
                out.push_str(&llm);
            }
        }

        // 结构化回执：调用方可直接读 `blocked` 决定是否终止，无需解析正文文本
        let severity = hits
            .iter()
            .map(|rf| rf.severity.clone())
            .max_by_key(|s| severity_rank(s));
        out.push_str(&format!(
            "\n\n{}",
            json!({
                "red_flags": hits.iter().map(|rf| json!({
                    "slug": rf.slug,
                    "label": rf.label,
                    "severity": rf.severity,
                    "advice": rf.advice,
                })).collect::<Vec<_>>(),
                "herb_safety": herb_hits,
                "blocked": blocked.is_some(),
                "severity": severity,
            })
        ));
        Ok(out)
    }
}

/// 严重级别排序：便于取「最高危」的那一条
fn severity_rank(s: &str) -> u8 {
    match s.trim().to_lowercase().as_str() {
        "critical" => 3,
        "high" => 2,
        "medium" => 1,
        _ => 0,
    }
}
