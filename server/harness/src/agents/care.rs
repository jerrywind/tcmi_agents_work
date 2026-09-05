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

        // 调护与辨证/立法/开方共用同一主证（见 `resolve_syndrome`）
        let slug = crate::agents::resolve_syndrome(&ctx.resources, messages, payload);

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

        // 调护条目同理必须进 system（同 T7.6）：
        // 拼在输出末尾对生成毫无影响，模型会另起一套说法。
        // H6：调护同样要感知证候置信度——证候是猜的，调护也该说得留有余地
        // （且调护条目本身会强化「这就是你的证」的错觉）。
        let uncertainty = crate::agents::syndrome_uncertainty_note(payload);

        let rule_block = if rule_part.is_empty() {
            String::new()
        } else {
            format!(
                "\n\n【本地调护库的确定性条目】\n{rule_part}\n\
                 请据此给出饮食/起居/情志调护，不要另起一套。"
            )
        };
        let system = if rule_block.is_empty() && uncertainty.is_empty() {
            ctx.resources.prompts.care.clone()
        } else {
            format!(
                "{}{}{}",
                ctx.resources.prompts.care, rule_block, uncertainty
            )
        };
        let llm = ctx
            .caller()
            .chat_with_tools(&system, messages, Capability::Care)
            .await?;
        Ok(format!("{llm}\n{rule_part}"))
    }
}
