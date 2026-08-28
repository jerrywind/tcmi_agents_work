//! 闻诊 sub-agent（听声音、嗅气味）
//!
//! 复刻 backend `app/agents/listening.py`。

use crate::agents::base::{chat_completion, AgentContext, SubAgent};
use crate::model::{Capability, Message};
use anyhow::Result;
use async_trait::async_trait;

pub struct ListeningAgent;

#[async_trait]
impl SubAgent for ListeningAgent {
    fn capability(&self) -> Capability {
        Capability::Listening
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],
        _payload: &serde_json::Value,
    ) -> Result<String> {
        let system = &ctx.resources.prompts.listening;
        let mut out = chat_completion(
            &ctx.llm,
            &ctx.config.llm_base_url,
            &ctx.config.llm_api_key,
            &ctx.config.model,
            system,
            messages,
        )
        .await?;

        if let Some(last) = messages.iter().rev().find(|m| m.role == "user") {
            let evidence = crate::agents::match_keywords(&ctx.resources, &last.content);
            if !evidence.is_empty() {
                out.push_str("\n\n[闻诊证据] ");
                out.push_str(&evidence.join("；"));
            }
        }
        Ok(out)
    }
}
