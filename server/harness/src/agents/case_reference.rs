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
use crate::resources::model::ResourceBundle;
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
        let hint = infer_syndromes_hint(&ctx.resources, messages, payload);

        // 典籍检索不可用时（T7.9），本步只能凭模型记忆「复述」医案，
        // 而复述出来的东西在报告里跟真医案长得一模一样：真实验证中出现过
        // 「白术附子汤」「黄连阿胶탕」（后者还混入了非中文字符）这类
        // 看似有出处、实则编造的内容，读者无从分辨。
        let rag_down = payload.get("rag_available").and_then(|v| v.as_bool()) == Some(false);
        let rag_note = if rag_down {
            "\n\n【重要：本次典籍检索不可用】未连接医案库，你没有检索到任何真实医案。\n\
             请**明确说明**这一点，不要用「某医案云」「《xx》载」的口吻复述——\
             那等于把生成内容伪装成真实病案。可改为说明同类证候的一般诊疗思路。"
        } else {
            ""
        };

        let base = &ctx.resources.prompts.case_reference;
        let system = if hint.is_empty() && rag_note.is_empty() {
            base.clone()
        } else {
            format!("{base}\n\n{hint}{rag_note}")
        };
        let llm = ctx
            .caller()
            .chat_with_tools(&system, messages, Capability::CaseReference)
            .await?;
        Ok(if rag_down {
            format!(
                "{llm}\n\n【依据说明】本次未连通典籍检索服务（RAG），\
                 以下医案未经检索核对，不作为诊疗依据。"
            )
        } else {
            llm
        })
    }
}

/// 拼一段「候选证候」提示：让检索有明确靶点
///
/// 本步执行在辨证**之前**（见模块文档），此时还没有 `assess()` 的结论，
/// 靶点只能来自文本推断——所以措辞必须说清这是方向、不是结论。
/// 否则模型会拿它当既成事实去检索相似医案，
/// 复述出来的「某医案云」与真实病案在报告里长得一模一样。
pub fn infer_syndromes_hint(
    res: &ResourceBundle,
    messages: &[Message],
    payload: &serde_json::Value,
) -> String {
    // 辨证环节已判定「未匹配到证候」时不再给推断靶点。
    // 这时再猜一个出来，等于把 H3 刚建立起来的诚实降级又绕回去了：
    // 辨证说「定不了」，医案参考却按猜的证把医案检索了一遍。
    let unmatched = payload.get("syndrome_matched").and_then(|v| v.as_bool()) == Some(false);

    let explicit = payload
        .get("syndrome")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());
    let inferred = if unmatched {
        Vec::new()
    } else {
        crate::agents::infer_syndrome_slug(res, messages)
            .into_iter()
            .take(2)
            .collect::<Vec<_>>()
    };

    let mut out = String::new();
    if let Some(s) = explicit {
        out.push_str(&format!("已知证候：{s}\n"));
    }
    if !inferred.is_empty() {
        let names: Vec<String> = inferred
            .iter()
            .map(|slug| {
                res.syndrome(slug)
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| slug.clone())
            })
            .collect();
        out.push_str(&format!(
            "待鉴别证候（**文本推断的初步方向，非正式辨证结论**）：{}\n",
            names.join("、")
        ));
    }
    if !out.is_empty() {
        out.insert_str(0, "【检索靶点】\n");
    }
    out
}
