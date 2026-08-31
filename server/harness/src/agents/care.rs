//! 调护 sub-agent（食疗 / 养生 / 生活调摄）
//!
//! 从 `care.yaml` 取确定性调护条目，再从 19 部养生摄生典籍与食疗本草里
//! 检索补充。默认**不在激活流程里**（见 `routing.yaml`），
//! 需要调护方案时启用 `full` 档位或显式指定。

use crate::agents::base::{AgentContext, SubAgent};
use crate::model::{Capability, Message};
use anyhow::Result;
use async_trait::async_trait;

pub struct CareAgent;

#[async_trait]
impl SubAgent for CareAgent {
    fn capability(&self) -> Capability {
        Capability::Care
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
            let cares = crate::knowledge::find_care(&ctx.resources, slug);
            if !cares.is_empty() {
                rule_part.push_str("【调护建议】\n");
                for c in &cares {
                    if let Some(items) = c.get("items").and_then(|v| v.as_array()) {
                        for it in items {
                            rule_part.push_str(&format!("- {}\n", it.as_str().unwrap_or("")));
                        }
                    }
                }
            }
        }

        let system = &ctx.resources.prompts.care;
        let llm = ctx
            .caller()
            .chat_with_tools(system, messages, Capability::Care)
            .await?;
        Ok(format!("{llm}\n{rule_part}"))
    }
}
