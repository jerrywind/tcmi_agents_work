//! 问诊 sub-agent
//!
//! 复刻 backend `app/agents/inquiry.py`：依据 `resources/questions.yaml`
//! 生成结构化问诊问题；依据已收集证据去重，逐步追问寒热/汗出/头身/二便等。

use crate::agents::base::{AgentContext, SubAgent};
use crate::model::{Capability, Message};
use crate::resources::model::Gender;
use anyhow::Result;
use async_trait::async_trait;

pub struct InquiryAgent;

#[async_trait]
impl SubAgent for InquiryAgent {
    fn capability(&self) -> Capability {
        Capability::Inquiry
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],
        payload: &serde_json::Value,
    ) -> Result<String> {
        // 0) 患者人群（T7.3）：`payload.gender` 由前端患者档案传入，
        //    此前本步的 payload 参数名是 `_payload`——传了也从未读过。
        let gender = Gender::from_payload(payload);

        // 1) 规则层：从问题库挑选尚未覆盖、且适用于本患者的提问
        let collected: String = messages
            .iter()
            .filter(|m| m.role == "user" || m.role == "assistant")
            .map(|m| m.content.clone())
            .collect::<Vec<_>>()
            .join("\n");

        let mut pending: Vec<&crate::resources::QuestionItem> = ctx
            .resources
            .questions
            .iter()
            .filter(|q| q.applies_to_gender(gender))
            .filter(|q| !q.evidence_keys.iter().any(|k| collected.contains(k)))
            .collect();
        pending.sort_by_key(|q| q.priority);

        let mut rule_part = String::new();
        if !pending.is_empty() {
            rule_part.push_str("【建议追问】\n");
            for q in pending.iter().take(6) {
                let cat = q.category.clone().unwrap_or_default();
                rule_part.push_str(&format!("- （{}）{}\n", cat, q.prompt));
            }
        }

        // 2) LLM 层：综合生成自然语言问诊（可用 tcm-inquiry 等技能）
        //
        // 性别必须显式告诉模型：规则层过滤掉了月经条目，模型若不知道
        // 患者是男性，仍会照着提示词里的「经带」二字自己追问一遍。
        let system = match gender {
            Gender::Male => format!(
                "{}\n\n【患者性别】男。禁止追问月经、带下、胎产等女性专属问题。",
                ctx.resources.prompts.inquiry
            ),
            Gender::Female => format!("{}\n\n【患者性别】女。", ctx.resources.prompts.inquiry),
            // 性别未采集时不加限定：宁可让模型多问一句，
            // 也不要在信息缺失时替它排除妇科鉴别线索。
            Gender::Unknown => ctx.resources.prompts.inquiry.clone(),
        };
        let llm_part = ctx
            .caller()
            .chat_with_tools(&system, messages, Capability::Inquiry)
            .await?;

        Ok(format!("{llm_part}\n{rule_part}"))
    }
}
