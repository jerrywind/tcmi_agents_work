//! 切诊 sub-agent（脉诊 + 体检数据）
//!
//! 复刻 backend `app/agents/palpation.py`：脉象由 LLM 描述，体检报告（PPG）
//! 由 `knowledge::ppg` 规则解析为数值并回灌。

use crate::agents::base::{chat_completion, AgentContext, SubAgent};
use crate::model::{Capability, Message};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

pub struct PalpationAgent;

#[async_trait]
impl SubAgent for PalpationAgent {
    fn capability(&self) -> Capability {
        Capability::Palpation
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],
        _payload: &serde_json::Value,
    ) -> Result<String> {
        let system = &ctx.resources.prompts.palpation;
        let mut out = chat_completion(
            &ctx.llm,
            &ctx.config.llm_base_url,
            &ctx.config.llm_api_key,
            &ctx.config.model,
            system,
            messages,
        )
        .await?;

        // PPG 解析：若用户消息含体检数值，结构化回灌
        if let Some(last) = messages.iter().rev().find(|m| m.role == "user") {
            let parsed = crate::knowledge::parse_ppg(&last.content);
            if let Some(obj) = parsed.as_object() {
                if !obj.is_empty() {
                    out.push_str(&format!("\n\n[体检数据解析] {}", json!(obj)));
                }
            }
        }
        Ok(out)
    }
}
