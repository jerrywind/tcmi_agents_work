//! 安全门 sub-agent
//!
//! 复刻 backend `app/agents/safety.py`：扫描红色警戒关键词（resources/safety.yaml），
//! 并校验处方用药安全（knowledge::herb_safety），给出转诊/警示建议。

use crate::agents::base::{chat_completion, AgentContext, SubAgent};
use crate::model::{Capability, Message};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

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
        payload: &serde_json::Value,
    ) -> Result<String> {
        let text: String = messages
            .iter()
            .map(|m| m.content.clone())
            .collect::<Vec<_>>()
            .join("\n");

        // 1) 红色警戒
        let mut alerts: Vec<String> = Vec::new();
        for rf in &ctx.resources.red_flags {
            if rf.keywords.iter().any(|kw| text.contains(kw.as_str())) {
                alerts.push(format!("[{}] {}", rf.severity, rf.advice));
            }
        }

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
            return Ok("【安全门】未触发红色警戒，处方用药无显著禁忌。".into());
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
            let llm = chat_completion(
                &ctx.llm,
                &ctx.config.llm_base_url,
                &ctx.config.llm_api_key,
                &ctx.config.model,
                &ctx.resources.prompts.safety,
                messages,
            )
            .await?;
            if !llm.trim().is_empty() {
                out.push('\n');
                out.push_str(&llm);
            }
        }

        // 结构化回执
        out.push_str(&format!(
            "\n\n{}",
            json!({"red_flags": alerts, "herb_safety": herb_hits})
        ));
        Ok(out)
    }
}
