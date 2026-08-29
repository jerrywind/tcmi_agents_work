//! 问诊 sub-agent
//!
//! 复刻 backend `app/agents/inquiry.py`：依据 `resources/questions.yaml`
//! 生成结构化问诊问题；依据已收集证据去重，逐步追问寒热/汗出/头身/二便等。

use crate::agents::base::{AgentContext, SubAgent};
use crate::model::{Capability, Message};
use anyhow::Result;
use async_trait::async_trait;

pub struct InquiryAgent;

#[async_trait]
impl SubAgent for InquiryAgent {
    fn capability(&self) -> Capability {
        Capability::Inquiry
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],
        _payload: &serde_json::Value,
    ) -> Result<String> {
        // 1) 规则层：从问题库挑选尚未覆盖的提问
        let collected: String = messages
            .iter()
            .filter(|m| m.role == "user" || m.role == "assistant")
            .map(|m| m.content.clone())
            .collect::<Vec<_>>()
            .join("\n");

        let mut pending: Vec<&crate::resources::QuestionItem> = ctx
            .resources
            .questions
            .iter()
            .filter(|q| !q.evidence_keys.iter().any(|k| collected.contains(k)))
            .collect();
        pending.sort_by_key(|q| q.priority);

        let mut rule_part = String::new();
        if !pending.is_empty() {
            rule_part.push_str("【建议追问】\n");
            for q in pending.iter().take(6) {
                let cat = q.category.clone().unwrap_or_default();
                rule_part.push_str(&format!("- （{}）{}\n", cat, q.prompt));
            }
        }

        // 2) LLM 层：综合生成自然语言问诊（可用 tcm-inquiry 等技能）
        let system = &ctx.resources.prompts.inquiry;
        let llm_part = ctx
            .caller()
            .chat_with_tools(system, messages, Capability::Inquiry)
            .await?;

        Ok(format!("{llm_part}\n{rule_part}"))
    }
}
