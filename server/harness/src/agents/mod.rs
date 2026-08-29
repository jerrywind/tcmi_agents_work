//! sub-agent 注册表与调度
//!
//! 七个 sub-agent 对应 backend 的 7 个 Capability：
//! inspection（望诊）/ listening（闻诊）/ inquiry（问诊）/ palpation（切诊）/
//! differentiation（辨证）/ safety（安全门）/ treatment（治疗）。
//!
//! 每个 agent 实现 `SubAgent` trait，由 `Registry` 按 capability 名查找并分发。

pub mod base;
pub mod differentiation;
pub mod inquiry;
pub mod inspection;
pub mod listening;
pub mod palpation;
pub mod safety;
pub mod treatment;

pub use base::{AgentContext, LlmCaller, SubAgent};

use crate::model::Capability;
use crate::resources::model::RedFlag;
use crate::resources::ResourceBundle;
use std::collections::HashMap;
use std::sync::Arc;

/// 规范顺序：望 → 闻 → 问 → 切 → 辨证 → 安全门 → 治疗
///
/// `Registry` 内部用 HashMap 存储，**迭代顺序不稳定**（Rust 的 HashMap
/// 使用随机化哈希，每次进程启动顺序都可能不同）。对外暴露能力清单时必须
/// 按此顺序，否则 `GET /agents` 每次重启返回的顺序都会变，
/// 前端分步展示与契约测试都会被随机顺序打乱。
const CAPABILITY_ORDER: [Capability; 7] = Capability::ALL;

/// 触发**中断**的红色警戒级别（T3.3）
///
/// high/critical 意味着需立即就医，此时继续输出治疗建议有延误风险，
/// 故安全门命中后直接终止后续步骤，并把结构化 `blocked` 标记回给调用方。
pub const BLOCKING_SEVERITIES: [&str; 2] = ["high", "critical"];

/// 该红色警戒是否需要中断流程
pub fn is_blocking(rf: &RedFlag) -> bool {
    BLOCKING_SEVERITIES.contains(&rf.severity.trim().to_lowercase().as_str())
}

/// 在文本中匹配 `resources/safety.yaml` 的红色警戒，返回命中条目（按资源顺序）
///
/// 供 `SafetyAgent` 与 `orchestrator` 共用：前者据此生成警示文案，
/// 后者据此决定是否中断后续步骤——两处必须是同一套判定，
/// 否则会出现「安全门告警了但流程照跑到治疗」的不一致。
pub fn detect_red_flags<'a>(res: &'a ResourceBundle, text: &str) -> Vec<&'a RedFlag> {
    res.red_flags
        .iter()
        .filter(|rf| rf.keywords.iter().any(|kw| text.contains(kw.as_str())))
        .collect()
}

/// 取最高危的、需要中断的红色警戒（无则返回 None）
pub fn blocking_red_flag<'a>(res: &'a ResourceBundle, text: &str) -> Option<&'a RedFlag> {
    detect_red_flags(res, text)
        .into_iter()
        .find(|rf| is_blocking(rf))
}

/// agent 注册表：capability -> 实例
#[derive(Clone)]
pub struct Registry {
    map: HashMap<Capability, Arc<dyn SubAgent>>,
}

impl Registry {
    pub fn new() -> Self {
        let mut map: HashMap<Capability, Arc<dyn SubAgent>> = HashMap::new();
        map.insert(
            Capability::Inspection,
            Arc::new(inspection::InspectionAgent),
        );
        map.insert(Capability::Listening, Arc::new(listening::ListeningAgent));
        map.insert(Capability::Inquiry, Arc::new(inquiry::InquiryAgent));
        map.insert(Capability::Palpation, Arc::new(palpation::PalpationAgent));
        map.insert(
            Capability::Differentiation,
            Arc::new(differentiation::DifferentiationAgent),
        );
        map.insert(Capability::Safety, Arc::new(safety::SafetyAgent));
        map.insert(Capability::Treatment, Arc::new(treatment::TreatmentAgent));
        Self { map }
    }

    pub fn get(&self, cap: Capability) -> Option<Arc<dyn SubAgent>> {
        self.map.get(&cap).cloned()
    }

    /// 已注册的能力列表，**按规范顺序返回**（不依赖 HashMap 迭代顺序）。
    pub fn capabilities(&self) -> Vec<Capability> {
        CAPABILITY_ORDER
            .iter()
            .copied()
            .filter(|c| self.map.contains_key(c))
            .collect()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

/// 在用户文本中匹配 `keywords.yaml` 的证据，返回命中的中文标签列表。
/// 供各四诊 agent 叠加「规则证据」，与 LLM 结论互相印证。
pub fn match_keywords(res: &crate::resources::ResourceBundle, text: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for ke in &res.keyword_evidence {
        let matched = ke.keywords.iter().any(|kw| text.contains(kw.as_str()));
        if matched {
            let syndrome_names: Vec<String> = ke
                .syndromes
                .iter()
                .filter_map(|s| res.syndrome(s).map(|x| x.name.clone()))
                .collect();
            let tail = if syndrome_names.is_empty() {
                String::new()
            } else {
                format!("（提示：{}）", syndrome_names.join("、"))
            };
            hits.push(format!("{}{}", ke.label, tail));
        }
    }
    hits
}

/// 从对话文本推断相关证候 slug 列表（降序，按关键词命中数计分）。
///
/// 通过 `keywords.yaml` 的证据 -> 证候映射统计得分，返回**所有得分 > 0**
/// 的证候 slug（降序）。保留多个候选以支持「兼证」场景，调用方可按需取
/// 首位或整体参与辨证。
pub fn infer_syndrome_slug(
    res: &crate::resources::ResourceBundle,
    messages: &[crate::model::Message],
) -> Vec<String> {
    let text: String = messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");

    let mut score: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for ke in &res.keyword_evidence {
        if ke.keywords.iter().any(|kw| text.contains(kw.as_str())) {
            for s in &ke.syndromes {
                *score.entry(s.clone()).or_insert(0) += 1;
            }
        }
    }
    let mut ranked: Vec<(String, usize)> = score.into_iter().filter(|(_, n)| *n > 0).collect();
    // 按得分降序：Reverse 配合 sort_by_key 比 sort_by 闭包更清晰
    ranked.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    ranked.into_iter().map(|(slug, _)| slug).collect()
}
