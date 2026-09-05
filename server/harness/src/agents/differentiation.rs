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

/// 主症命中一条的证据量（H2）
///
/// 主症是定证的必要条件，权重最高。
const W_KEY: f64 = 1.0;

/// 次症命中一条的证据量（H2）
///
/// 次症是旁证：多个证候共有的非特异表现（乏力、纳呆、失眠…）不该与
/// 「恶寒重发热轻」「脉浮紧」这类强特异表现同权，否则**症状表越长的
/// 证候越容易赢**，凑次症就能压过命中主症。
const W_MINOR: f64 = 0.4;

/// 舌象 / 脉象命中的证据量
///
/// 舌脉往往是寒热虚实鉴别的关键，权重与症状同级。
const W_SIGN: f64 = 1.0;

/// 关键词证据的证据量（H1）
///
/// **只计症状/舌脉没算过的线索**。此前症状与 `keywords.yaml` 对同一批词
/// 各计一次（症状 1.0 + 关键词 0.5），而多数证候的 keywords 与其 symptoms
/// 高度重叠，于是证据量被系统性放大约 1.5 倍，置信度虚高——
/// 「命中一句口苦」就攒出 1.5 分，收敛判定（min_confidence 0.6）被轻易突破。
const W_KEYWORD: f64 = 0.5;

/// 置信度满分所需的证据量
const FULL_EVIDENCE: f64 = 5.0;

/// 单条矛盾证据扣减的证据量
const CONFLICT_PENALTY: f64 = 0.5;

/// 进入候选集的最低置信度（低于此值视为偶然命中，不予呈现）
const MIN_CONFIDENCE: f64 = 0.2;

/// 成为主证的最低证据量（H3「孤证不立」）
///
/// 满足主症必备只说明「方向对」，一条孤证不足以定证：
/// 「头痛」是风寒感冒的主症，但头痛可见于十几个证候；
/// 「发热」是风热犯肺的主症，而膀胱湿热的患者也发热。
/// 1.5 对应「1 条主症 + 至少 1 条佐证（次症 0.4 / 舌脉 1.0）」——
/// 单条主症（1.0）或「主症 + 无佐证」都停在候选与 `near` 里，不作结论。
const PRIMARY_MIN_SCORE: f64 = 1.5;

/// 兼证门槛：置信度达到主证该比例的候选，视为与主证并存的兼证
const CONCURRENT_RATIO: f64 = 0.6;

/// 未匹配时最多呈现几条「最接近但未达主症必备」的候选
const MAX_NEAR_MISS: usize = 2;

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
    /// 是否满足「主症必备」（H3）：至少命中一条主症。
    ///
    /// 只有 `qualified` 的候选才能成为主证或兼证。未定义主症的旧式证候
    /// 恒为 `true`——资源没填不该让整个库失效。
    pub qualified: bool,
    /// 未命中的主症（H3）。
    ///
    /// 用于把「差在哪」说清楚：不只是「没匹配上」，而是「最像 X，但缺
    /// 便溏、脘腹胀满这些主症」。模型拿到这个才说得出「证据不足」，
    /// 前端也能据此提示患者补充。
    pub missing_key_symptoms: Vec<String>,
}

/// 结构化辨证结论（T4.1 主证 / T4.2 兼证）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DifferentiationResult {
    /// 主证（满足主症必备、且证据量最高者；无合格候选时为 `None`）
    pub primary: Option<SyndromeAssessment>,
    /// 兼证：与主证并存、且证据量达标的其他证候（按证据量降序）
    pub concurrent: Vec<SyndromeAssessment>,
    /// 基于主证的传变提示
    pub transformations: Vec<String>,
    /// 是否匹配到明确证候（`primary.is_some()` 的镜像，便于调用方直读）
    pub matched: bool,
    /// 全部有命中的候选（按证据量降序，含未合格的）。
    ///
    /// 收敛判定要拿**真实第二名**算鉴别度（H5）——此前用的是
    /// `concurrent.first()`，而兼证已被「score ≥ 主证×0.6」过滤过，
    /// 第二名一旦被滤掉，`margin` 就退化成主证自身分数，鉴别度判定形同虚设。
    pub ranked: Vec<SyndromeAssessment>,
    /// 未匹配时「最接近但未达主症必备」的候选（H3）
    ///
    /// 与 `matched=false` 一起给模型看：知道「最像 X，但缺主症 Y」，
    /// 比只看到「未匹配」更接近真实中医的判断过程，
    /// 也让它有依据说出「证据不足」，而不是硬编一个证名。
    pub near: Vec<SyndromeAssessment>,
}

impl DifferentiationResult {
    /// 供 LLM 参考的一行提示（把规则初筛结论喂给模型，避免它凭空起证名）
    pub fn brief(&self) -> String {
        let Some(p) = &self.primary else {
            return self.unmatched_brief();
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

    /// 未匹配到明确证候时的提示（H3）
    ///
    /// 措辞是刻意的：**明确授权模型说「不知道」**。
    /// 原来只说「证据不足，请自行辨证」，模型在只有 6 个证候可挑的语境下
    /// 仍会挑一个最像的交差——因为报告里必须有一个证名。
    /// 补上「最接近的是谁、缺哪条主症」后，它才有依据说「不足以定证」。
    fn unmatched_brief(&self) -> String {
        let tail = "请依据四诊信息自行辨证；\
                    若证据确实不足，请**直接说明无法定证**，\
                    不要勉强归入某一证候，更不要按猜测的证候开方。";
        if self.near.is_empty() {
            return format!(
                "规则初筛：未匹配到任何证候（四诊信息未命中库内证候的典型表现）。{tail}"
            );
        }
        let cands: Vec<String> = self
            .near
            .iter()
            .map(|n| {
                if n.missing_key_symptoms.is_empty() {
                    format!("{}（证据量 {:.2}）", n.name, n.score)
                } else {
                    format!(
                        "{}（证据量 {:.2}，但缺主症：{}）",
                        n.name,
                        n.score,
                        n.missing_key_symptoms.join("、")
                    )
                }
            })
            .collect();
        format!(
            "规则初筛：未匹配到明确证候（库内候选均未满足主症必备条件）。\
             最接近的是：{}。{tail}",
            cands.join("；")
        )
    }

    /// 渲染成 Markdown（作为该步骤正文的「结构化辨证」小节）
    pub fn render(&self) -> String {
        let Some(p) = &self.primary else {
            return format!(
                "【结构化辨证】{}\n\n【接近但未达主症必备】{}\n",
                self.unmatched_brief(),
                if self.near.is_empty() {
                    "（无）".to_string()
                } else {
                    self.near
                        .iter()
                        .map(|n| {
                            format!(
                                "{}（证据量 {:.2}，缺主症：{}）",
                                n.name,
                                n.score,
                                if n.missing_key_symptoms.is_empty() {
                                    "（未定义主症）".to_string()
                                } else {
                                    n.missing_key_symptoms.join("、")
                                }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("；")
                }
            );
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

    // H3 主症必备 + 孤证不立：只有 `qualified` 且证据量达标的候选才配当主证。
    //
    // 此前取 `scored.first()`——在 6 个证候里必选其一，**必然**产出主证，
    // 而 MIN_CONFIDENCE 只有 0.2，命中一条非特异次症（如「乏力」）就够了。
    // 于是库外的证候（肾阳虚、食积…）会被判成库内最像的那个，
    // 置信度 0.3 也照样被 `lock_syndrome` 钉给治疗期，开方步按错证开方，
    // 而报告里看不出任何异常。现在没有合格候选就是没有，如实说不知道。
    let primary_idx = scored.iter().position(|s| {
        s.qualified && s.score >= PRIMARY_MIN_SCORE && s.confidence >= MIN_CONFIDENCE
    });
    let primary = primary_idx.map(|i| scored[i].clone());

    let (skip, threshold) = match primary_idx {
        Some(i) => (i + 1, scored[i].score * CONCURRENT_RATIO),
        None => (0usize, f64::MAX),
    };
    let concurrent: Vec<SyndromeAssessment> = scored
        .iter()
        .skip(skip)
        .filter(|s| s.qualified && s.confidence >= MIN_CONFIDENCE && s.score >= threshold)
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

    // H3：未匹配时把「最接近谁、差在哪」一并交出去。
    // 包含两种未达标：缺主症，或只有孤证（有主症但证据量不足）。
    let near: Vec<SyndromeAssessment> = if primary.is_none() {
        scored
            .iter()
            .filter(|s| !s.qualified || s.score < PRIMARY_MIN_SCORE)
            .take(MAX_NEAR_MISS)
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    let matched = primary.is_some();

    DifferentiationResult {
        primary,
        concurrent,
        transformations,
        matched,
        ranked: scored,
        near,
    }
}

/// 对单个证候打分；一条证据都没命中时返回 `None`（不进入候选集）
fn score_syndrome(res: &ResourceBundle, s: &Syndrome, text: &str) -> Option<SyndromeAssessment> {
    let mut supporting: Vec<String> = Vec::new();
    // 参与矛盾判定的**原词**（症状 / 舌象 / 脉象片段，不含关键词标签）
    let mut terms: Vec<String> = Vec::new();
    let mut raw = 0.0f64;
    let mut key_hits = 0usize;
    let mut missing_key_symptoms: Vec<String> = Vec::new();

    // H2：主症（权重 1.0）——命中任一条即满足「主症必备」
    for sym in s.key_symptoms() {
        if text.contains(sym.as_str()) {
            supporting.push(sym.clone());
            terms.push(sym.clone());
            raw += W_KEY;
            key_hits += 1;
        } else {
            missing_key_symptoms.push(sym.clone());
        }
    }
    // H2：次症（权重 0.4）——只作旁证，凑数凑不出主证
    for sym in s.minor_symptoms() {
        if text.contains(sym.as_str()) {
            supporting.push(sym.clone());
            terms.push(sym.clone());
            raw += W_MINOR;
        }
    }
    if let Some(seg) = match_segment(s.tongue.as_deref(), text) {
        supporting.push(format!("舌象：{seg}"));
        terms.push(seg);
        raw += W_SIGN;
    }
    if let Some(seg) = match_segment(s.pulse.as_deref(), text) {
        supporting.push(format!("脉象：{seg}"));
        terms.push(seg);
        raw += W_SIGN;
    }
    // H1：关键词证据**只补症状表之外的线索**。
    //
    // `keywords.yaml` 的多数条目与证候自身的 symptoms 高度重叠
    // （脾胃湿热 10 个关键词里有 9 个就是它的症状），于是同一句
    // 「口苦」先计 1.0 再计 0.5。去重方式：关键词与任一已命中的
    // 症状/舌脉原词互为子串即视为同一表现，不再重复计分。
    for ke in &res.keyword_evidence {
        if !ke.syndromes.iter().any(|x| x == &s.slug) {
            continue;
        }
        let Some(kw) = ke.keywords.iter().find(|k| text.contains(k.as_str())) else {
            continue;
        };
        if terms
            .iter()
            .any(|t| t.contains(kw.as_str()) || kw.contains(t.as_str()))
        {
            continue;
        }
        supporting.push(ke.label.clone());
        raw += W_KEYWORD;
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

    // H3 主症必备。证候未定义主症（旧格式）时恒为 true：
    // 资源没填是资源的问题，不该让整个库一起失效。
    let qualified = s.key_symptoms().is_empty() || key_hits > 0;

    Some(SyndromeAssessment {
        slug: s.slug.clone(),
        name: s.name.clone(),
        confidence,
        supporting,
        conflicting,
        pathogenesis: s.pathogenesis.clone(),
        score,
        qualified,
        missing_key_symptoms,
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
