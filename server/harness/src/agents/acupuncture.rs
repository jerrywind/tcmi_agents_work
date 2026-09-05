//! 针灸外治 sub-agent
//!
//! 从 44 部针灸典籍（刺法灸法 / 腧穴考证 / 经络理论 / 时间针法 / 推拿按摩）
//! 检索取穴与手法依据。默认**不在激活流程里**（见 `routing.yaml` 的
//! `compatible` / `standard` 档位），需要针灸方案时显式启用。
//!
//! 为什么与开方分开：针灸与方药是两套治疗体系，合在「治疗」一步里
//! 模型常常只给方不给穴，或两者都给得很浅。

use crate::agents::base::{AgentContext, SubAgent};
use crate::model::{Capability, Message};
use crate::resources::model::ResourceBundle;
use anyhow::Result;
use async_trait::async_trait;

pub struct AcupunctureAgent;

#[async_trait]
impl SubAgent for AcupunctureAgent {
    fn capability(&self) -> Capability {
        Capability::Acupuncture
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],
        payload: &serde_json::Value,
    ) -> Result<String> {
        // 规则层：**没有取穴规则**——取穴高度依赖具体证候与经络辨证，
        // 硬编码一张「证候 → 穴位」表反而会误导（同一证候的取穴随流派、
        // 兼证、体质而变）。这部分保留给模型在针灸典籍里检索后综合。
        //
        // 但**证候、病机、经络与它的置信度必须注入**。
        //
        // 这是 H6 的漏网：本步属治疗期（`phase_of` 归入治疗段），
        // 拿到的 payload 里有 `syndrome` / `syndrome_matched` / `syndrome_locked`，
        // 此前却被 `_payload` 整个忽略。于是辨证判定「未匹配到证候」时，
        // 针灸照样一本正经开穴——而**针灸是有创操作**，
        // 未定证就给出穴位处方，风险不比开错方小。
        let syndrome_block = syndrome_block(&ctx.resources, messages, payload);

        // 典籍检索是否接通（T7.9）：与开方步同一套口径。
        // 本步的输出高度依赖典籍原文，RAG 不可达时若不明说，
        // 模型会凭记忆「复述」出处，读报告的人无从分辨真伪。
        let rag_down = payload.get("rag_available").and_then(|v| v.as_bool()) == Some(false);
        // 只讲检索来源相关的约束。安全要求（须由执业医师操作）放进
        // `prompts.yaml` 的常驻部分——那是**任何时候都成立**的，
        // 不该只在 RAG 断了的时候才提醒。
        let rag_note = if rag_down {
            "\n\n【重要：本次典籍检索不可用】\n\
             连接针灸典籍库（RAG）失败，你**没有**检索到任何原文。\n\
             1. 只能依据你的既有知识取穴与论述手法；\n\
             2. **不得杜撰书名与篇名**——无法确认出处的，一律写「出处待核」。"
        } else {
            ""
        };

        let uncertainty = crate::agents::syndrome_uncertainty_note(payload);

        let base = &ctx.resources.prompts.acupuncture;
        let system = if syndrome_block.is_empty() && rag_note.is_empty() && uncertainty.is_empty() {
            base.clone()
        } else {
            format!("{base}{syndrome_block}{rag_note}{uncertainty}")
        };
        let llm = ctx
            .caller()
            .chat_with_tools(&system, messages, Capability::Acupuncture)
            .await?;
        Ok(if rag_down {
            format!(
                "{llm}\n\n【依据说明】本次未连通典籍检索服务（RAG），\
                 以上取穴与手法的典籍出处未经检索核对，不作为诊疗依据。"
            )
        } else {
            llm
        })
    }
}

/// 把辨证结论（主证 / 经络 / 病机 / 治则）拼成取穴依据
///
/// 与 `resolve_syndrome` 同源，因此看到的主证与立法、开方、调护完全一致；
/// 证候未锁定时它拿到的是 `None`，本步便不会按一个可能是错的证去取穴。
///
/// 取 `&ResourceBundle` 而非 `&AgentContext`，是为了能在 `tests/behavior.rs`
/// 里直接断言——H6 那条教训的延续：这类「某步看不看得到结论」的逻辑
/// 必须能测，否则漏网了也没人知道。
pub fn syndrome_block(
    res: &ResourceBundle,
    messages: &[Message],
    payload: &serde_json::Value,
) -> String {
    let Some(slug) = crate::agents::resolve_syndrome(res, messages, payload) else {
        return String::new();
    };
    let Some(s) = res.syndrome(&slug) else {
        return String::new();
    };

    let mut out = format!("\n\n【当前主证】{}", s.name);
    if let Some(m) = &s.meridian {
        out.push_str(&format!("\n【涉及经络/脏腑】{m}"));
    }
    if let Some(p) = &s.pathogenesis {
        out.push_str(&format!("\n【病机】{p}"));
    }
    if !s.principles.is_empty() {
        out.push_str(&format!("\n【治则】{}", s.principles.join("、")));
    }
    out.push_str("\n请据此取穴配伍；取穴思路须与上述主证、病机一致。");
    out
}
