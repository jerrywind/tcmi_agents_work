//! 辨证 sub-agent
//!
//! 复刻 backend `app/agents/differentiation.py`：综合四诊信息，
//! 借助 `resources/syndromes.yaml` 证候库做规则初筛 + LLM 辨证，
//! 输出证名、病机、治法与传变提示。

use crate::agents::base::{chat_completion, AgentContext, SubAgent};
use crate::model::{Capability, Message};
use anyhow::Result;
use async_trait::async_trait;

pub struct DifferentiationAgent;

#[async_trait]
impl SubAgent for DifferentiationAgent {
    fn capability(&self) -> Capability {
        Capability::Differentiation
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],
        _payload: &serde_json::Value,
    ) -> Result<String> {
        // 规则初筛：把四诊文本与证候库症状做命中度排序
        let text: String = messages
            .iter()
            .map(|m| m.content.clone())
            .collect::<Vec<_>>()
            .join("\n");

        let mut scored: Vec<(String, usize)> = ctx
            .resources
            .syndromes
            .iter()
            .map(|s| {
                let hits = s
                    .symptoms
                    .iter()
                    .filter(|sym| text.contains(sym.as_str()))
                    .count();
                (s.name.clone(), hits)
            })
            .collect();
        scored.retain(|(_, n)| *n > 0);
        scored.sort_by(|a, b| b.1.cmp(&a.1));

        let rule_summary = if scored.is_empty() {
            String::new()
        } else {
            let top = scored
                .iter()
                .take(3)
                .map(|(name, n)| format!("{name}(命中{n})"))
                .collect::<Vec<_>>()
                .join("、");
            format!("【证候库初筛】倾向：{top}\n")
        };

        // LLM 辨证
        let system = &ctx.resources.prompts.differentiation;
        let llm = chat_completion(
            &ctx.llm,
            &ctx.config.llm_base_url,
            &ctx.config.llm_api_key,
            &ctx.config.model,
            system,
            messages,
        )
        .await?;

        // 传变提示
        let transform_note = if let Some(best) = scored.first() {
            let from_slug = ctx
                .resources
                .syndromes
                .iter()
                .find(|s| s.name == best.0)
                .map(|s| s.slug.clone());
            if let Some(slug) = from_slug {
                let ts: Vec<String> = ctx
                    .resources
                    .transformations
                    .iter()
                    .filter(|t| t.from == slug)
                    .map(|t| t.label.clone())
                    .collect();
                if !ts.is_empty() {
                    format!("\n\n【传变提示】{}", ts.join("；"))
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        Ok(format!("{rule_summary}{llm}{transform_note}"))
    }
}
