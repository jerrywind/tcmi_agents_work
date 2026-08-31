//! 立法 sub-agent（治则治法）
//!
//! 辨证之后、用药开方之前，先定**治则**（汗、吐、下、和、温、清、消、补）。
//!
//! 为什么单独一步：「理法方药」里「法」承上启下——没有治则就直接开方，
//! 模型容易跳到具体方名而说不清为什么用它。有了治则，开方便有标尺可校验。
//!
//! 规则层给出 `syndromes.yaml` 里的 `principles`（确定性），
//! LLM 层再结合病机与古籍依据展开。

use crate::agents::base::{AgentContext, SubAgent};
use crate::model::{Capability, Message};
use anyhow::Result;
use async_trait::async_trait;

pub struct StrategyAgent;

#[async_trait]
impl SubAgent for StrategyAgent {
    fn capability(&self) -> Capability {
        Capability::Strategy
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],
        payload: &serde_json::Value,
    ) -> Result<String> {
        let mut rule_part = String::new();

        let slug = payload
            .get("syndrome")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                crate::agents::infer_syndrome_slug(&ctx.resources, messages)
                    .into_iter()
                    .next()
            });

        if let Some(slug) = &slug {
            if let Some(s) = ctx.resources.syndrome(slug) {
                rule_part.push_str(&format!("【证候】{}（{}）\n", s.name, slug));
                if let Some(p) = &s.pathogenesis {
                    rule_part.push_str(&format!("【病机】{p}\n"));
                }
                if !s.principles.is_empty() {
                    rule_part.push_str(&format!("【治则】{}\n", s.principles.join("、")));
                }
            }
        }

        let system = &ctx.resources.prompts.strategy;
        let llm = ctx
            .caller()
            .chat_with_tools(system, messages, Capability::Strategy)
            .await?;
        Ok(format!("{llm}\n{rule_part}"))
    }
}
