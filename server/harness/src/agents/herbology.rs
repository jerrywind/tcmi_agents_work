//! 用药 sub-agent（本草）
//!
//! 在开方**之前**定药：药性、炮制、配伍、剂量。
//!
//! 知识域是 58 部本草专著（药性理论 / 炮制制剂 / 配伍归经 / 经典本草）。
//! 古籍里的剂量与炮制法带有时代局限，故本步只做「药」层面的依据与提示，
//! 最终剂量仍以现代药典为准——这条约束写在 `prompts.herbology` 里。

use crate::agents::base::{AgentContext, SubAgent};
use crate::model::{Capability, Message};
use anyhow::Result;
use async_trait::async_trait;

pub struct HerbologyAgent;

#[async_trait]
impl SubAgent for HerbologyAgent {
    fn capability(&self) -> Capability {
        Capability::Herbology
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],
        payload: &serde_json::Value,
    ) -> Result<String> {
        let mut rule_part = String::new();

        // 1) 若调用方给定了具体药味，先做配伍禁忌与妊娠禁忌校验（确定性规则）
        if let Some(herbs) = payload.get("herbs").and_then(|h| h.as_array()) {
            let list: Vec<String> = herbs
                .iter()
                .filter_map(|h| h.as_str().map(|s| s.to_string()))
                .collect();
            if !list.is_empty() {
                let pregnant = payload
                    .get("pregnant")
                    .and_then(|p| p.as_bool())
                    .unwrap_or(false);
                let hits = crate::knowledge::check_herb_safety(&list, pregnant);
                if hits.is_empty() {
                    rule_part.push_str("【用药安全】未触发配伍/妊娠禁忌。\n");
                } else {
                    rule_part.push_str("【用药安全】\n");
                    for h in &hits {
                        rule_part.push_str(&format!("- {}\n", h.detail));
                    }
                }
            }
        }

        // 2) 候选方剂的组成，作为「药」层面的起点
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
            let formulas = crate::knowledge::find_formula(&ctx.resources, slug);
            let comps: Vec<String> = formulas
                .iter()
                .filter_map(|f| {
                    f.get("composition")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str())
                                .collect::<Vec<_>>()
                                .join("、")
                        })
                        .filter(|s| !s.is_empty())
                })
                .collect();
            if !comps.is_empty() {
                rule_part.push_str(&format!("【候选药物】{}\n", comps.join("；")));
            }
        }

        let system = &ctx.resources.prompts.herbology;
        let llm = ctx
            .caller()
            .chat_with_tools(system, messages, Capability::Herbology)
            .await?;
        Ok(format!("{llm}\n{rule_part}"))
    }
}
