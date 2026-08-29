//! 辨证 sub-agent
//!
//! 复刻 backend `app/agents/differentiation.py`：综合四诊信息，
//! 借助 `resources/syndromes.yaml` 证候库做规则初筛 + LLM 辨证，
//! 输出证名、病机、治法与传变提示。
//!
//! ## T4.1 / T4.2：结构化辨证
//!
//! 规则初筛从此前「Top3 一行提示」升级为**结构化结论**：
//! - 每个候选证候给出**置信度**、**支持证据**与**矛盾证据**；
//! - 与主证并存的**兼证**一并列出，不再只报第一名；
//! - 结果既渲染成 Markdown（供分步展示），也经 `SubAgent::structured`
//!   随 `/chat`、`POST /agents` 原样返回，供前端做卡片化呈现（T4.2）。
//!
//! 计算是**确定性纯函数**（`assess`），不依赖 LLM：
//! 同一份语料必得同一份结论，可直接写回归测试，也让 LLM 有据可依。

use crate::agents::base::{AgentContext, SubAgent};
use crate::model::{Capability, Message};
use crate::resources::{model::Syndrome, ResourceBundle};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 置信度满分所需的证据量。
///
/// 一条症状/舌象/脉象计 1.0，一条关键词证据计 0.5；
/// 攒够 5 条证据即视为证据充分（置信度 1.0）。
const FULL_EVIDENCE: f64 = 5.0;

/// 单条矛盾证据扣减的证据量
const CONFLICT_PENALTY: f64 = 0.5;

/// 进入候选集的最低置信度（低于此值视为偶然命中，不予呈现）
const MIN_CONFIDENCE: f64 = 0.2;

/// 兼证门槛：置信度达到主证该比例的候选，视为与主证并存的兼证
const CONCURRENT_RATIO: f64 = 0.6;

/// 舌象/脉象文案的分隔符（证候库里写作「舌淡红，苔薄白」）
const TERM_SEPARATORS: [char; 2] = ['，', ','];

/// 单个证候的结构化评估（T4.1）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyndromeAssessment {
    pub slug: String,
    pub name: String,
    /// 置信度 0~1（保留两位小数）
    pub confidence: f64,
    /// 支持证据：命中的症状 / 舌象 / 脉象 / 关键词证据标签
    pub supporting: Vec<String>,
    /// 矛盾证据：语料中出现了与本证候命中表现**相反**的表现
    pub conflicting: Vec<String>,
    /// 病机（来自证候库）
    pub pathogenesis: Option<String>,
    /// 证据量（诊断用：排序、兼证判定；不用于前端展示）
    pub score: f64,
}

/// 结构化辨证结论（T4.1 主证 / T4.2 兼证）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DifferentiationResult {
    /// 主证（证据量最高者；证据不足时为 `None`）
    pub primary: Option<SyndromeAssessment>,
    /// 兼证：与主证并存、且证据量达标的其他证候（按证据量降序）
    pub concurrent: Vec<SyndromeAssessment>,
    /// 基于主证的传变提示
    pub transformations: Vec<String>,
}

impl DifferentiationResult {
    /// 供 LLM 参考的一行提示（把规则初筛结论喂给模型，避免它凭空起证名）
    pub fn brief(&self) -> String {
        let Some(p) = &self.primary else {
            return "规则初筛：证据不足，未匹配到明确证候，请依据四诊信息自行辨证。".to_string();
        };
        if self.concurrent.is_empty() {
            format!(
                "规则初筛：主证「{}」（置信度 {:.2}，依据：{}）。请结合四诊复核，可修正。",
                p.name,
                p.confidence,
                p.supporting.join("、")
            )
        } else {
            let names: Vec<String> = self
                .concurrent
                .iter()
                .map(|c| format!("{}（{:.2}）", c.name, c.confidence))
                .collect();
            format!(
                "规则初筛：主证「{}」（置信度 {:.2}，依据：{}）；兼证：{}。请结合四诊复核，可修正。",
                p.name,
                p.confidence,
                p.supporting.join("、"),
                names.join("、")
            )
        }
    }

    /// 渲染成 Markdown（作为该步骤正文的「结构化辨证」小节）
    pub fn render(&self) -> String {
        let Some(p) = &self.primary else {
            return "【结构化辨证】四诊信息不足，未匹配到明确证候。".to_string();
        };
        let mut out = String::from("【结构化辨证】\n");
        out.push_str(&render_one("主证", p));
        for c in &self.concurrent {
            out.push('\n');
            out.push_str(&render_one("兼证", c));
        }
        if !self.transformations.is_empty() {
            out.push_str(&format!(
                "\n【传变提示】{}",
                self.transformations.join("；")
            ));
        }
        out
    }
}

fn render_one(kind: &str, s: &SyndromeAssessment) -> String {
    let mut out = format!("- {kind}：{}（置信度 {:.2}）\n", s.name, s.confidence);
    out.push_str(&format!("  支持证据：{}\n", join_or_default(&s.supporting)));
    out.push_str(&format!(
        "  矛盾证据：{}\n",
        join_or_default(&s.conflicting)
    ));
    if let Some(p) = &s.pathogenesis {
        out.push_str(&format!("  病机：{p}\n"));
    }
    out
}

fn join_or_default(items: &[String]) -> String {
    if items.is_empty() {
        "（无）".to_string()
    } else {
        items.join("、")
    }
}

pub struct DifferentiationAgent;

#[async_trait]
impl SubAgent for DifferentiationAgent {
    fn capability(&self) -> Capability {
        Capability::Differentiation
    }

    /// 结构化输出（T4.1 / T4.2）：把规则初筛结论原样交回给调用方
    ///
    /// 与 [`assess`] 用的是同一份纯函数结果：LLM 只负责在此之上润色与修正，
    /// 不参与结构化字段的计算，因此结构化输出是确定的、可回归的。
    fn structured(&self, ctx: &AgentContext, messages: &[Message]) -> Option<Value> {
        serde_json::to_value(assess(&ctx.resources, messages)).ok()
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],
        _payload: &Value,
    ) -> Result<String> {
        // 规则初筛：结构化辨证（置信度 / 支持证据 / 矛盾证据 / 兼证）
        let result = assess(&ctx.resources, messages);

        // LLM 辨证（可用 tcm-reference / tcm-kb 等技能查证候库）。
        // 把规则初筛结论附在系统提示里，让模型有据可依而非凭空起证名。
        let system = format!(
            "{}\n\n{}",
            ctx.resources.prompts.differentiation,
            result.brief()
        );
        let llm = ctx
            .caller()
            .chat_with_tools(&system, messages, Capability::Differentiation)
            .await?;

        Ok(format!("{}\n\n{}", result.render(), llm))
    }
}

/// 结构化辨证：对全部证候打分，产出主证 + 兼证 + 传变提示。
///
/// 纯函数、无副作用、不依赖 LLM：
/// - 证据量 `score` = 症状命中 + 舌象命中 + 脉象命中 + 0.5 × 关键词证据命中
///   − 0.5 × 矛盾证据数（下限 0）；
/// - 置信度 `confidence` = `min(1, score / FULL_EVIDENCE)`；
/// - 主证 = 证据量最高者（同分保持证候库顺序，便于预测）；
/// - 兼证 = 其余候选中置信度达标、且证据量 ≥ 主证 × `CONCURRENT_RATIO` 者。
pub fn assess(res: &ResourceBundle, messages: &[Message]) -> DifferentiationResult {
    let text: String = messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");

    let mut scored: Vec<SyndromeAssessment> = res
        .syndromes
        .iter()
        .filter_map(|s| score_syndrome(res, s, &text))
        .collect();
    // sort_by 是稳定排序：同分时保持证候库顺序，输出可预测
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let primary = scored
        .first()
        .filter(|s| s.confidence >= MIN_CONFIDENCE)
        .cloned();

    let (skip, threshold) = match &primary {
        Some(p) => (1usize, p.score * CONCURRENT_RATIO),
        None => (0usize, f64::MAX),
    };
    let concurrent: Vec<SyndromeAssessment> = scored
        .iter()
        .skip(skip)
        .filter(|s| s.confidence >= MIN_CONFIDENCE && s.score >= threshold)
        .cloned()
        .collect();

    let transformations = match &primary {
        Some(p) => res
            .transformations
            .iter()
            .filter(|t| t.from == p.slug)
            .map(|t| t.label.clone())
            .collect(),
        None => Vec::new(),
    };

    DifferentiationResult {
        primary,
        concurrent,
        transformations,
    }
}

/// 对单个证候打分；一条证据都没命中时返回 `None`（不进入候选集）
fn score_syndrome(res: &ResourceBundle, s: &Syndrome, text: &str) -> Option<SyndromeAssessment> {
    let mut supporting: Vec<String> = Vec::new();
    // 参与矛盾判定的**原词**（症状 / 舌象 / 脉象片段，不含关键词标签）
    let mut terms: Vec<String> = Vec::new();
    let mut raw = 0.0f64;

    for sym in &s.symptoms {
        if text.contains(sym.as_str()) {
            supporting.push(sym.clone());
            terms.push(sym.clone());
            raw += 1.0;
        }
    }
    if let Some(seg) = match_segment(s.tongue.as_deref(), text) {
        supporting.push(format!("舌象：{seg}"));
        terms.push(seg);
        raw += 1.0;
    }
    if let Some(seg) = match_segment(s.pulse.as_deref(), text) {
        supporting.push(format!("脉象：{seg}"));
        terms.push(seg);
        raw += 1.0;
    }
    for ke in &res.keyword_evidence {
        if ke.syndromes.iter().any(|x| x == &s.slug)
            && ke.keywords.iter().any(|k| text.contains(k.as_str()))
        {
            supporting.push(ke.label.clone());
            raw += 0.5;
        }
    }

    if raw == 0.0 {
        return None;
    }

    let mut conflicting: Vec<String> = Vec::new();
    for term in &terms {
        for c in &res.contradictions {
            if let Some(opp) = c.opposite_in(term, text) {
                let opp = opp.to_string();
                if !conflicting.contains(&opp) {
                    conflicting.push(opp);
                }
            }
        }
    }

    let score = (raw - CONFLICT_PENALTY * conflicting.len() as f64).max(0.0);
    let confidence = ((score / FULL_EVIDENCE).min(1.0) * 100.0).round() / 100.0;

    Some(SyndromeAssessment {
        slug: s.slug.clone(),
        name: s.name.clone(),
        confidence,
        supporting,
        conflicting,
        pathogenesis: s.pathogenesis.clone(),
        score,
    })
}

/// 舌象/脉象是短语（如「舌淡红，苔薄白」），按分隔符拆开逐段匹配，
/// 命中任一段即算命中。证候库用「，」连接多个特征，整体匹配几乎不可能命中。
fn match_segment(field: Option<&str>, text: &str) -> Option<String> {
    let field = field?.trim();
    if field.is_empty() {
        return None;
    }
    field
        .split(|c| TERM_SEPARATORS.contains(&c))
        .map(|p| p.trim())
        .find(|p| !p.is_empty() && text.contains(p))
        .map(|p| p.to_string())
}
