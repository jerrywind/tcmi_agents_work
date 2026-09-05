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

        // 证候来自「证候锁定」：辨证步的主证（编排器注入）或调用方显式给定；
        // 只在都没有时才退回文本推断。立法依据的主证必须与辨证一致，
        // 否则后面「据证立法」立的就是另一个证的法。
        let slug = crate::agents::resolve_syndrome(&ctx.resources, messages, payload);

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

        // 同开方步：证候库给出的治则必须进 system。
        // 治则是「法」，是后面「方」的标尺；模型看不到就会自造治则名
        // （如把「辛温解表」写成「发散风寒」这类自造词）。
        // H6：证候没锁定（未定证 / 置信度不足）时必须让模型知道，
        // 否则它会把推断出的证候当定论，一板一眼地立出「法」来。
        let uncertainty = crate::agents::syndrome_uncertainty_note(payload);

        // 没有规则结论时不拼这个小节：标题写着「来自证候库，确定性」，
        // 底下却是空的，等于拿一个空的权威标题去压模型。
        let rule_block = if rule_part.is_empty() {
            String::new()
        } else {
            format!(
                "\n\n【证候与治则（来自证候库，确定性）】\n{rule_part}\n\
                 请据此立法；治则名沿用上述表述，不要自造同义说法。"
            )
        };
        let system = if rule_block.is_empty() && uncertainty.is_empty() {
            ctx.resources.prompts.strategy.clone()
        } else {
            format!(
                "{}{}{}",
                ctx.resources.prompts.strategy, rule_block, uncertainty
            )
        };
        let llm = ctx
            .caller()
            .chat_with_tools(&system, messages, Capability::Strategy)
            .await?;
        Ok(format!("{llm}\n{rule_part}"))
    }
}
