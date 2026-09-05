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
        // 开方依据的证候必须就是辨证步认定的主证（见 `resolve_syndrome`）。
        // 此前本步自己从对话重新猜一遍，猜出来的证与辨证结论不一致时，
        // 就出现「辨证脾胃湿热、开方龙胆泻肝汤」这种方证不对口。
        let syndrome_slug = crate::agents::resolve_syndrome(&ctx.resources, messages, payload);

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
                    let source = f.get("source").and_then(|v| v.as_str()).unwrap_or("");
                    rule_part.push_str(&format!("- {name}（出自{source}）：{comp}"));
                    if !usage.is_empty() {
                        rule_part.push_str(&format!("\n  用法：{usage}"));
                    }
                    if !caution.is_empty() {
                        rule_part.push_str(&format!("\n  禁忌：{caution}"));
                    }
                    rule_part.push('\n');
                }
                rule_part.push_str(
                    "\n以上为本地方剂库的确定性结果：采用其中方剂时，组成须与上面记载\
                     完全一致（不得凭记忆增减药味），确需加减须逐味说明理由。\n\
                     请再用 tcm-rag 在方书中检索，补充更贴切或可备选的方剂，并说明取舍。\n",
                );
            }
        }

        // 候选方剂必须**喂进 system**，不能只拼在输出末尾。
        //
        // 拼在末尾（`format!("{llm}\n{rule_part}")`）对生成毫无影响——
        // 模型生成时看不到自己后面会接什么，于是只能凭记忆拟方：
        // 真实验证里它自造了一个并不存在的「半夏白术天花粉汤」，
        // 而库里正确的连朴饮/三仁汤就摆在输出末尾，白放着没用上。
        //
        // 辨证步早就是对的：它把规则结论经 `brief()` 放进 system，
        // 所以主证判得准。开方步把这个模式补上。
        // 典籍检索是否接通（T7.9）：`rag_available` 由编排器注入
        // （见 `orchestrator::with_rag_status`）。单步调用不带此字段时按可用处理，
        // 不改变既有行为。
        let rag_down = payload.get("rag_available").and_then(|v| v.as_bool()) == Some(false);

        // RAG 不可用时必须堵住「编出处」：模型不知道自己没查到，照样写
        // 「出自《xxx》」。真实验证里就把龙胆泻肝汤标成了出自《伤寒论》
        // （该方实出自《医方集解》）——读报告的人没有能力分辨这是编的。
        let rag_note = if rag_down {
            "\n\n【重要：本次典籍检索不可用】\n\
             连接典籍库（RAG）失败，你**没有**检索到任何原文。\n\
             1. 只能依据上面本地方剂库的记载与你的既有知识拟方；\n\
             2. **不得杜撰书名与篇名**——此前出现过把龙胆泻肝汤标为出自\
             《伤寒论》这类错误；\n\
             3. 无法确认出处的，一律写「出处待核」，不要给一个看起来像的书名。"
        } else {
            ""
        };

        // H6：证候没锁定时，规则层已经判定「不可信」，必须同步给生成侧。
        // 否则规则层说归说，模型照样开出一副有模有样的方——
        // 而读报告的人看不出证候是猜的。
        let uncertainty = crate::agents::syndrome_uncertainty_note(payload);

        let system = if rule_part.is_empty() && rag_note.is_empty() && uncertainty.is_empty() {
            ctx.resources.prompts.prescription.clone()
        } else {
            let who = syndrome_slug
                .as_deref()
                .and_then(|s| ctx.resources.syndrome(s))
                .map(|s| format!("（主证：{}）", s.name))
                .unwrap_or_default();
            let candidates = if rule_part.is_empty() {
                String::new()
            } else {
                format!(
                    "\n\n【本地方剂库候选方剂{}】\n{}\n\
                     要求：主方优先从上述候选方中选择，采用时组成须与上面记载完全一致。\
                     若候选方均不适用，须先逐条说明不适用的理由，再给出自拟方，\
                     并标明其为「经验方」而非古籍成方。",
                    who, rule_part
                )
            };
            format!(
                "{}{}{}{}",
                ctx.resources.prompts.prescription, candidates, rag_note, uncertainty
            )
        };
        let llm = ctx
            .caller()
            .chat_with_tools(&system, messages, Capability::Prescription)
            .await?;

        // 给读报告的人也留一句：有没有典籍支撑，不该靠猜。
        if rag_down {
            rule_part.push_str(
                "\n【依据说明】本次未连通典籍检索服务（RAG），以上方剂的典籍出处\
                 未经检索核对；书名与篇名请以本地方剂库记载为准。\n",
            );
        }

        // 药味核对（T7.4）：模型输出里若列了库载方剂，比对组成是否对得上。
        // 这是确定性的字符串比对，专门兜「经方药味记错」这类提示词压不住的幻觉。
        let checks = crate::knowledge::check_composition(&ctx.resources, &llm);
        if !checks.is_empty() {
            rule_part.push_str("\n【药味核对提示（自动比对，非最终结论）】\n");
            for c in &checks {
                rule_part.push_str(&format!("- {c}\n"));
            }
        }

        Ok(format!("{llm}\n{rule_part}"))
    }
}
