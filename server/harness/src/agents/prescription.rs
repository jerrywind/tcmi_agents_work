//! 开方 sub-agent（**治疗阶段的最后一步**）
//!
//! 依据已确立的治则与证候，从方书检索方剂并给出处方。
//!
//! 为什么从「治疗」里拆出来：原 `treatment` 一步同时干开方、用药、调护、
//! 外治，每样都浅。开方是治疗的落点，值得单独一步——也只有它配得上
//! 110 部方书的检索域（方剂汇编 / 专科方书 / 经验验方 / 成药标准 /
//! 急救方书 / 方论阐释），科室还随辨证结果动态收窄（儿科就看儿科方书）。
//!
//! 与 `herbology` 的分工：本步定「方」，用药步定「药」
//! （药性、炮制、配伍、剂量）。

use crate::agents::base::{AgentContext, SubAgent};
use crate::model::{Capability, Message};
use anyhow::Result;
use async_trait::async_trait;

pub struct PrescriptionAgent;

#[async_trait]
impl SubAgent for PrescriptionAgent {
    fn capability(&self) -> Capability {
        Capability::Prescription
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],
        payload: &serde_json::Value,
    ) -> Result<String> {
        let syndrome_slug = payload
            .get("syndrome")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                crate::agents::infer_syndrome_slug(&ctx.resources, messages)
                    .into_iter()
                    .next()
            });

        let mut rule_part = String::new();
        if let Some(slug) = &syndrome_slug {
            let formulas = crate::knowledge::find_formula(&ctx.resources, slug);
            if !formulas.is_empty() {
                rule_part.push_str("【推荐方剂】\n");
                for f in &formulas {
                    let name = f.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let comp = f
                        .get("composition")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str())
                                .collect::<Vec<_>>()
                                .join("、")
                        })
                        .unwrap_or_default();
                    let usage = f.get("usage").and_then(|v| v.as_str()).unwrap_or("");
                    let caution = f.get("caution").and_then(|v| v.as_str()).unwrap_or("");
                    rule_part.push_str(&format!("- {name}：{comp}"));
                    if !usage.is_empty() {
                        rule_part.push_str(&format!("\n  用法：{usage}"));
                    }
                    if !caution.is_empty() {
                        rule_part.push_str(&format!("\n  禁忌：{caution}"));
                    }
                    rule_part.push('\n');
                }
                rule_part.push_str(
                    "\n以上为本地方剂库的确定性结果；请再用 tcm-rag 在方书中检索，\
                     补充更贴切或可备选的方剂，并说明取舍。\n",
                );
            }
        }

        let system = &ctx.resources.prompts.prescription;
        let llm = ctx
            .caller()
            .chat_with_tools(system, messages, Capability::Prescription)
            .await?;
        Ok(format!("{llm}\n{rule_part}"))
    }
}
