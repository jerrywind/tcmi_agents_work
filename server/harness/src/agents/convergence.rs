//! 反馈式辨证的**收敛判定与追问生成**
//!
//! ## 为什么需要它
//!
//! 望闻问切采集到的信息常常不足以定证——患者只说「咳嗽两天」，
//! 此时硬着头皮辨证，主证置信度可能只有 0.2，却照样往下走到开方。
//! 真实中医靠**反复追问**把信息补齐，本模块就是把这件事自动化：
//!
//! ```text
//! Phase A 采集（望闻问切）→ Phase B 辨证 → 收敛？
//!                              ↑                │
//!                              └── 不收敛：生成追问，等用户回答 ──┘
//! ```
//!
//! ## 设计要点
//!
//! - **全部是纯函数**：不调 LLM、不联网，同一份输入必得同一份结论，
//!   因此可以直接写回归测试；
//! - **单轮内不空转**：真实信息只能来自用户，故单轮内**不会**让模型
//!   「再想想」——那样只会产生幻觉。不收敛就返回追问，等下一轮携带新信息进来；
//! - **追问是确定性的**：鉴别追问由证候库的**症状差集**自动推导，
//!   未覆盖追问来自 `questions.yaml`，都不靠模型编造。
//!
//! ## 收敛三条件（同时满足才算收敛）
//!
//! 1. **置信度** `confidence ≥ min_confidence`：主证证据够不够；
//! 2. **鉴别度** `margin ≥ margin`：主证与第二名拉开没有——
//!    两者咬得很近说明鉴别不清，再往下走就是猜；
//! 3. **覆盖率** `coverage ≥ coverage`：必采信息（舌象/脉象/寒热等）采集到了没有。
//!
//! 兜底：达到 `max_rounds` 强制放行，标 `converged=false` —— 这保证
//! 「最终一定有结论」，而不是把用户卡在无限追问里。

use crate::agents::differentiation::{assess, DifferentiationResult};
use crate::model::Message;
use crate::resources::ResourceBundle;
use serde::{Deserialize, Serialize};

/// 反馈式辨证 loop 的可调参数
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LoopConfig {
    /// 最大追问轮数，达到后强制以当前最佳证候放行
    pub max_rounds: u8,
    /// 主证置信度门槛
    pub min_confidence: f64,
    /// 主证与次证的证据量差门槛
    pub margin: f64,
    /// 必采信息覆盖率门槛
    pub coverage: f64,
    /// priority ≤ 此项的题目算「必采」
    pub required_priority: u8,
    /// 单轮最多追问几条
    pub max_questions: usize,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_rounds: 3,
            min_confidence: 0.6,
            margin: 1.0,
            coverage: 0.8,
            required_priority: 6,
            max_questions: 5,
        }
    }
}

/// 一条待补充采集的追问
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingQuestion {
    pub slug: String,
    /// 向用户提问的文案
    pub text: String,
    /// 为什么要问（前端可展示，也让用户理解配合的意义）
    pub reason: String,
    /// 来源：`discriminator` 鉴别追问 / `uncovered` 未覆盖 / `syndrome` 证候补全
    pub source: String,
    /// 该信息该由哪个采集 agent 负责（后续轮只跑必要的 agent）
    pub agent: String,
    pub priority: u8,
}

/// 收敛判定结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Convergence {
    /// 当前轮次（从 1 起）
    pub round: u8,
    pub converged: bool,
    /// 是否因达到轮次上限而强制放行
    pub forced: bool,
    pub confidence: f64,
    pub margin: f64,
    pub coverage: f64,
    /// 主证 slug（无主证时为空）
    pub primary_slug: String,
    pub pending_questions: Vec<PendingQuestion>,
}

impl Convergence {
    /// 该跑哪些采集 agent（后续轮据此只跑必要的）。
    ///
    /// 返回空表示「照常全跑」——用于首轮或追问里没标明 agent 的情况。
    pub fn collection_agents(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for q in &self.pending_questions {
            if !q.agent.is_empty() && !out.contains(&q.agent) {
                out.push(q.agent.clone());
            }
        }
        out
    }
}

/// 判定是否收敛，并生成待补充采集的追问。
///
/// `round` 由调用方从 `payload.round` 传入（前端持有，harness 无状态）。
pub fn evaluate(
    res: &ResourceBundle,
    messages: &[Message],
    cfg: &LoopConfig,
    round: u8,
) -> Convergence {
    let diff = assess(res, messages);
    let corpus: String = messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");

    let (confidence, margin, primary_slug) = confidence_and_margin(&diff);
    let coverage = required_coverage(res, &corpus, cfg.required_priority);

    let mut conv = Convergence {
        round: round.max(1),
        converged: false,
        forced: false,
        confidence,
        margin,
        coverage,
        primary_slug,
        pending_questions: Vec::new(),
    };

    let enough =
        confidence >= cfg.min_confidence && margin >= cfg.margin && coverage >= cfg.coverage;

    if enough {
        conv.converged = true;
        return conv;
    }
    if round.max(1) >= cfg.max_rounds {
        // 兜底：强制放行，但如实标注「未收敛」
        conv.forced = true;
        conv.converged = true;
        return conv;
    }

    conv.pending_questions = build_questions(res, &diff, &corpus, cfg);
    conv
}

/// 主证置信度，以及主证与次证的证据量差
fn confidence_and_margin(diff: &DifferentiationResult) -> (f64, f64, String) {
    let Some(p) = &diff.primary else {
        return (0.0, 0.0, String::new());
    };
    // H5：取**真实第二名**，而不是兼证第一名。
    //
    // `concurrent` 已被「score ≥ 主证 × CONCURRENT_RATIO」过滤过：
    // 第二名一旦被滤掉，`concurrent.first()` 为 None，margin 就退化成
    // 主证自身分数——于是「没有兼证」恒等于「鉴别度达标」，
    // 鉴别度判定形同虚设。真实第二名才能反映「有没有和第二名拉开」。
    let second = diff
        .ranked
        .iter()
        .find(|s| s.slug != p.slug)
        .map(|s| s.score)
        .unwrap_or(0.0);
    (p.confidence, (p.score - second).max(0.0), p.slug.clone())
}

/// 必采信息的覆盖率：priority ≤ 门槛的题目里，证据已出现在对话中的比例
fn required_coverage(res: &ResourceBundle, corpus: &str, required_priority: u8) -> f64 {
    let required: Vec<_> = res
        .questions
        .iter()
        .filter(|q| q.priority <= required_priority)
        .collect();
    if required.is_empty() {
        return 1.0;
    }
    let hit = required
        .iter()
        .filter(|q| q.evidence_keys.iter().any(|k| corpus.contains(k.as_str())))
        .count();
    hit as f64 / required.len() as f64
}

/// 生成追问，按来源优先级：鉴别 > 未覆盖 > 证候补全
fn build_questions(
    res: &ResourceBundle,
    diff: &DifferentiationResult,
    corpus: &str,
    cfg: &LoopConfig,
) -> Vec<PendingQuestion> {
    let mut out: Vec<PendingQuestion> = Vec::new();

    // 1) 鉴别追问：主证与次证的**症状差集**自动推导
    out.extend(discriminator_questions(res, diff, corpus));

    // 2) 未覆盖追问：问题库里证据尚未采集的，按 priority
    let mut uncovered: Vec<PendingQuestion> = res
        .questions
        .iter()
        .filter(|q| !q.evidence_keys.iter().any(|k| corpus.contains(k.as_str())))
        .map(|q| PendingQuestion {
            slug: q.slug.clone(),
            text: q.prompt.clone(),
            reason: format!(
                "尚缺「{}」方面的信息",
                q.category.clone().unwrap_or_default()
            ),
            source: "uncovered".into(),
            agent: q.agent.clone().unwrap_or_default(),
            priority: q.priority,
        })
        .collect();
    uncovered.sort_by_key(|q| q.priority);
    out.extend(uncovered);

    // 3) 证候补全：主证里尚未提及的典型症状。
    //
    // H3 配套：未匹配到主证时以 `near` 中最接近的候选为目标——
    // 此时**最该问的就是它缺的那几条主症**，因为缺主症正是没匹配上的原因。
    let target = diff.primary.as_ref().or_else(|| diff.near.first());
    if let Some(p) = target {
        if let Some(s) = res.syndrome(&p.slug) {
            let missing_keys: Vec<&String> = s
                .key_symptoms()
                .iter()
                .filter(|sym| !corpus.contains(sym.as_str()))
                .collect();
            let missing: Vec<&String> = if missing_keys.is_empty() {
                s.all_symptoms()
                    .into_iter()
                    .filter(|sym| !corpus.contains(sym.as_str()))
                    .take(3)
                    .collect()
            } else {
                missing_keys.into_iter().take(3).collect()
            };
            if !missing.is_empty() {
                let list: Vec<String> = missing.iter().map(|s| s.to_string()).collect();
                let is_primary = diff.primary.is_some();
                out.push(PendingQuestion {
                    slug: format!("{}_confirm", p.slug),
                    text: format!("是否出现以下表现：{}？", list.join("、")),
                    reason: if is_primary {
                        format!("核实主证「{}」的典型表现", s.name)
                    } else {
                        format!("尚未定证，最接近「{}」，需核实其主症", s.name)
                    },
                    source: "syndrome".into(),
                    agent: "inquiry".into(),
                    priority: u8::MAX,
                });
            }
        }
    }

    out.truncate(cfg.max_questions);
    out
}

/// 鉴别追问：取主证与次证的症状差集。
///
/// 「想喝水吗」能区分风热与风寒——这正是鉴别诊断的精髓。差集是**从证候库
/// 自动推导**的，不需要人工维护「与某证的鉴别要点」字段。
fn discriminator_questions(
    res: &ResourceBundle,
    diff: &DifferentiationResult,
    corpus: &str,
) -> Vec<PendingQuestion> {
    // H3 配套：未匹配到主证时，改用 `near` 里最接近的两个候选做鉴别——
    // 此时正是最需要「问一句就能分开」的时候（如问「怕冷还是怕热」）。
    let (first, second) = match (&diff.primary, diff.concurrent.first()) {
        (Some(p), Some(q)) => (p.slug.clone(), q.slug.clone()),
        (Some(p), None) => match diff.ranked.iter().find(|s| s.slug != p.slug) {
            Some(q) => (p.slug.clone(), q.slug.clone()),
            None => return Vec::new(),
        },
        (None, _) => match (diff.near.first(), diff.near.get(1)) {
            (Some(a), Some(b)) => (a.slug.clone(), b.slug.clone()),
            _ => return Vec::new(),
        },
    };
    let (Some(a), Some(b)) = (res.syndrome(&first), res.syndrome(&second)) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    // 主证独有 + 次证独有，各取最能区分的一条
    for (only_in, other_name) in [(a, &b.name), (b, &a.name)] {
        let diff_syms: Vec<&String> = only_in
            .all_symptoms()
            .into_iter()
            .filter(|s| !corpus.contains(s.as_str()))
            .collect();
        if let Some(sym) = diff_syms.first() {
            out.push(PendingQuestion {
                slug: format!("disc_{}_{}", only_in.slug, slugify(sym)),
                text: format!("是否出现「{sym}」？"),
                reason: format!("鉴别「{}」与「{}」", a.name, other_name),
                source: "discriminator".into(),
                agent: "inquiry".into(),
                priority: 0, // 鉴别追问永远最优先
            });
        }
    }
    out
}

/// 症状短语 -> 稳定后缀（中文直接取前 8 字，够区分即可）
fn slugify(s: &str) -> String {
    s.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Message;

    fn msg(s: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: s.to_string(),
        }
    }

    /// 主诉过于简短时不应收敛——这正是 loop 要解决的場景
    #[test]
    fn short_complaint_does_not_converge() {
        let res = crate::resources::load(std::path::Path::new("resources"))
            .expect("测试需能加载 resources/");
        let cfg = LoopConfig::default();
        let c = evaluate(&res, &[msg("我咳嗽两天了")], &cfg, 1);
        assert!(!c.converged);
        assert!(c.confidence < cfg.min_confidence || c.coverage < cfg.coverage);
        assert!(!c.pending_questions.is_empty());
    }

    /// 达到轮次上限必须强制放行，否则用户会被无限追问
    #[test]
    fn max_rounds_forces_convergence() {
        let res = crate::resources::load(std::path::Path::new("resources")).unwrap();
        let cfg = LoopConfig::default();
        let c = evaluate(&res, &[msg("我咳嗽两天了")], &cfg, cfg.max_rounds);
        assert!(c.converged);
        assert!(c.forced);
        assert!(c.pending_questions.is_empty());
    }

    /// 追问必须带上 agent，后续轮才能只跑必要的采集 agent
    #[test]
    fn questions_carry_agent() {
        let res = crate::resources::load(std::path::Path::new("resources")).unwrap();
        let cfg = LoopConfig::default();
        let c = evaluate(&res, &[msg("我咳嗽两天了")], &cfg, 1);
        assert!(c.pending_questions.iter().any(|q| !q.agent.is_empty()));
        assert!(!c.collection_agents().is_empty());
    }
}
