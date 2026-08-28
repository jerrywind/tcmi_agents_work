//! 治疗 sub-agent
//!
//! 复刻 backend `app/agents/treatment.py`：依据已辨证的证候 slug，
//! 从 `resources/formulas.yaml` / `care.yaml` 检索方剂与调护，做用药安全校验，
//! 再经 LLM 综合成药膳食疗/方剂/外治方案。

use crate::agents::base::{chat_completion, AgentContext, SubAgent};
use crate::model::{Capability, Message};
use anyhow::Result;
use async_trait::async_trait;

pub struct TreatmentAgent;

#[async_trait]
impl SubAgent for TreatmentAgent {
    fn capability(&self) -> Capability {
        Capability::Treatment
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],
        payload: &serde_json::Value,
    ) -> Result<String> {
        // 1) 规则检索：证候 slug 来自 payload，或从消息文本推断
        let syndrome_slug = payload
            .get("syndrome")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .or_else(|| crate::agents::infer_syndrome_slug(&ctx.resources, messages).into_iter().next());

        let mut rule_part = String::new();
        if let Some(slug) = &syndrome_slug {
            let formulas = crate::knowledge::find_formula(&ctx.resources, slug);
            let cares = crate::knowledge::find_care(&ctx.resources, slug);
            if !formulas.is_empty() {
                rule_part.push_str("【推荐方剂】\n");
                for f in &formulas {
                    rule_part.push_str(&format!(
                        "- {}：{}\n",
                        f.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                        f.get("composition")
                            .and_then(|v| v.as_array())
                            .map(|a| a
                                .iter()
                                .filter_map(|x| x.as_str())
                                .collect::<Vec<_>>()
                                .join("、"))
                            .unwrap_or_default()
                    ));
                }
            }
            if !cares.is_empty() {
                rule_part.push_str("【调护建议】\n");
                for c in &cares {
                    if let Some(items) = c.get("items").and_then(|v| v.as_array()) {
                        for it in items {
                            rule_part.push_str(&format!(
                                "- {}\n",
                                it.as_str().unwrap_or("")
                            ));
                        }
                    }
                }
            }
        }

        // 2) 用药安全
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
            if !hits.is_empty() {
                rule_part.push_str("【用药安全】\n");
                for h in &hits {
                    rule_part.push_str(&format!("- {}\n", h.detail));
                }
            }
        }

        // 3) LLM 综合
        let system = &ctx.resources.prompts.treatment;
        let llm = chat_completion(
            &ctx.llm,
            &ctx.config.llm_base_url,
            &ctx.config.llm_api_key,
            &ctx.config.model,
            system,
            messages,
        )
        .await?;

        Ok(format!("{llm}\n{rule_part}"))
    }
}
