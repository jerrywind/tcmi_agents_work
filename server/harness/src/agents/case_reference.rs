//! 医案参考 sub-agent
//!
//! 在 48 部临证医案里检索**相似病案**，供辨证参照。
//!
//! 为什么单独一步：医案是中医最有价值的经验载体，但它是「别人的病历」，
//! 与「辨证规则」是两类东西。混进辨证步会让模型把个案当通则；
//! 单独一步则能在辨证**之前**给出参照，让辨证有前例可依。
//!
//! 知识域（见 `resources/rag_scopes.yaml`）：体裁=临证医案 / 功能=临证实录，
//! 科室由辨证结果动态注入——儿科病就看儿科医案。

use crate::agents::base::{AgentContext, SubAgent};
use crate::model::{Capability, Message};
use anyhow::Result;
use async_trait::async_trait;

pub struct CaseReferenceAgent;

#[async_trait]
impl SubAgent for CaseReferenceAgent {
    fn capability(&self) -> Capability {
        Capability::CaseReference
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],
        payload: &serde_json::Value,
    ) -> Result<String> {
        // 规则层：把已知的候选证候交给模型，让它据此检索相似医案，
        // 而不是拿整段主诉去模糊匹配。
        let hint = infer_syndromes_hint(ctx, messages, payload);

        let system = &ctx.resources.prompts.case_reference;
        let system = if hint.is_empty() {
            system.clone()
        } else {
            format!("{system}\n\n{hint}")
        };
        let llm = ctx
            .caller()
            .chat_with_tools(&system, messages, Capability::CaseReference)
            .await?;
        Ok(llm)
    }
}

/// 拼一段「候选证候」提示：让检索有明确靶点
pub(crate) fn infer_syndromes_hint(
    ctx: &AgentContext,
    messages: &[Message],
    payload: &serde_json::Value,
) -> String {
    let explicit = payload
        .get("syndrome")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());
    let inferred = crate::agents::infer_syndrome_slug(&ctx.resources, messages)
        .into_iter()
        .take(2)
        .collect::<Vec<_>>();

    let mut out = String::new();
    if let Some(s) = explicit {
        out.push_str(&format!("已知证候：{s}\n"));
    }
    if !inferred.is_empty() {
        let names: Vec<String> = inferred
            .iter()
            .map(|slug| {
                ctx.resources
                    .syndrome(slug)
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| slug.clone())
            })
            .collect();
        out.push_str(&format!("待鉴别证候：{}\n", names.join("、")));
    }
    if !out.is_empty() {
        out.insert_str(0, "【检索靶点】\n");
    }
    out
}
