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
        // 与辨证步、立法步共用同一个主证（见 `resolve_syndrome`），
        // 候选药物才可能落在正确的方上。
        let slug = crate::agents::resolve_syndrome(&ctx.resources, messages, payload);
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

        // 与开方步同理：候选组成必须进 system。
        // 拼在输出末尾等于没给模型看，它会另起炉灶讨论一批别的药。
        // H6：证候未锁定时同样的提示——证候是猜的，药就不该讲得斩钉截铁。
        let uncertainty = crate::agents::syndrome_uncertainty_note(payload);

        let rule_block = if rule_part.is_empty() {
            String::new()
        } else {
            format!(
                "\n\n【本地知识库的确定性结果】\n{rule_part}\n\
                 请据此讨论药性、炮制与配伍，不要另起一批药物。"
            )
        };
        let system = if rule_block.is_empty() && uncertainty.is_empty() {
            ctx.resources.prompts.herbology.clone()
        } else {
            format!(
                "{}{}{}",
                ctx.resources.prompts.herbology, rule_block, uncertainty
            )
        };
        let llm = ctx
            .caller()
            .chat_with_tools(&system, messages, Capability::Herbology)
            .await?;
        Ok(format!("{llm}\n{rule_part}"))
    }
}
