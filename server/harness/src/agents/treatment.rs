//! 治疗 sub-agent
//!
//! 复刻 backend `app/agents/treatment.py`：依据已辨证的证候 slug，
//! 从 `resources/formulas.yaml` / `care.yaml` 检索方剂与调护，做用药安全校验，
//! 再经 LLM 综合成药膳食疗/方剂/外治方案。

use crate::agents::base::{AgentContext, SubAgent};
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
        // 1) 规则检索：证候以辨证步的主证为准（见 `resolve_syndrome`）
        let syndrome_slug = crate::agents::resolve_syndrome(&ctx.resources, messages, payload);

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
                            rule_part.push_str(&format!("- {}\n", it.as_str().unwrap_or("")));
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

        // 3) LLM 综合（可用专属 tcm-formula / tcm-care，以及全局 tcm-kb / tcm-diet / tcm-rag）
        //
        // 规则结果必须进 system（同 T7.6）。本步是**兼容档**的一步到位版本，
        // T7.6 当年只改了拆分后的立法/用药/开方三步，漏了这里——
        // 于是走兼容档时模型照样看不到库载方剂：真实验证里它把「脾胃湿热」
        // 讲成了「腹胀腹痛腹泻」并开了参苓白术散（那是脾虚湿困的方），
        // 而规则层给出的连朴饮/三仁汤就摆在输出末尾，白放着。
        // H6：兼容档同样要感知证候置信度（T7.12 的教训——漏了旧流程步，
        // 走兼容档的用户拿到的仍是旧行为，而兼容档是配置支持的档位）。
        let uncertainty = crate::agents::syndrome_uncertainty_note(payload);

        let rule_block = if rule_part.is_empty() {
            String::new()
        } else {
            format!(
                "\n\n【本地方剂库与调护库的确定性结果】\n{rule_part}\n\
                 要求：方剂优先从中选择，采用时组成须与上面记载一致；\
                 确需加减须逐味说明理由。"
            )
        };
        let system = if rule_block.is_empty() && uncertainty.is_empty() {
            ctx.resources.prompts.treatment.clone()
        } else {
            format!(
                "{}{}{}",
                ctx.resources.prompts.treatment, rule_block, uncertainty
            )
        };
        let llm = ctx
            .caller()
            .chat_with_tools(&system, messages, Capability::Treatment)
            .await?;

        Ok(format!("{llm}\n{rule_part}"))
    }
}
