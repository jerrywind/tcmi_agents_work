//! 望诊 sub-agent
//!
//! 复刻 backend `app/agents/inspection.py`：观察神色形态、舌象（图片或文字描述），
//! 通过 LLM 抽取结构化特征，并叠加 `resources/keywords.yaml` 的证据匹配。

use crate::agents::base::{AgentContext, SubAgent};
use crate::model::{Capability, Message};
use anyhow::Result;
use async_trait::async_trait;

pub struct InspectionAgent;

#[async_trait]
impl SubAgent for InspectionAgent {
    fn capability(&self) -> Capability {
        Capability::Inspection
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],
        _payload: &serde_json::Value,
    ) -> Result<String> {
        let system = &ctx.resources.prompts.inspection;
        // 用 chat_with_tools 而非 chat_completion：让模型能调用 tcm-vision 等技能。
        // 该 capability 无可用技能时，内部会自动退化为普通补全。
        let mut out = ctx
            .caller()
            .chat_with_tools(system, messages, Capability::Inspection)
            .await?;

        // 关键词证据叠加：从最新用户消息匹配舌象/面色证据
        if let Some(last) = messages.iter().rev().find(|m| m.role == "user") {
            let evidence = crate::agents::match_keywords(&ctx.resources, &last.content);
            if !evidence.is_empty() {
                out.push_str("\n\n[望诊证据] ");
                out.push_str(&evidence.join("；"));
            }
        }
        Ok(out)
    }
}
