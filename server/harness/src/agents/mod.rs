//! sub-agent 注册表与调度
//!
//! 七个 sub-agent 对应 backend 的 7 个 Capability：
//! inspection（望诊）/ listening（闻诊）/ inquiry（问诊）/ palpation（切诊）/
//! differentiation（辨证）/ safety（安全门）/ treatment（治疗）。
//!
//! 每个 agent 实现 `SubAgent` trait，由 `Registry` 按 capability 名查找并分发。

pub mod base;
pub mod differentiation;
pub mod inspection;
pub mod inquiry;
pub mod listening;
pub mod palpation;
pub mod safety;
pub mod treatment;

pub use base::{AgentContext, SubAgent};
pub use base::{chat_completion, chat_with_tools};

use crate::model::Capability;
use std::collections::HashMap;
use std::sync::Arc;

/// agent 注册表：capability -> 实例
#[derive(Clone)]
pub struct Registry {
    map: HashMap<Capability, Arc<dyn SubAgent>>,
}

impl Registry {
    pub fn new() -> Self {
        let mut map: HashMap<Capability, Arc<dyn SubAgent>> = HashMap::new();
        map.insert(Capability::Inspection, Arc::new(inspection::InspectionAgent));
        map.insert(Capability::Listening, Arc::new(listening::ListeningAgent));
        map.insert(Capability::Inquiry, Arc::new(inquiry::InquiryAgent));
        map.insert(Capability::Palpation, Arc::new(palpation::PalpationAgent));
        map.insert(Capability::Differentiation, Arc::new(differentiation::DifferentiationAgent));
        map.insert(Capability::Safety, Arc::new(safety::SafetyAgent));
        map.insert(Capability::Treatment, Arc::new(treatment::TreatmentAgent));
        Self { map }
    }

    pub fn get(&self, cap: Capability) -> Option<Arc<dyn SubAgent>> {
        self.map.get(&cap).cloned()
    }

    pub fn capabilities(&self) -> Vec<Capability> {
        self.map.keys().copied().collect()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

/// 在用户文本中匹配 `keywords.yaml` 的证据，返回命中的中文标签列表。
/// 供各四诊 agent 叠加「规则证据」，与 LLM 结论互相印证。
pub fn match_keywords(
    res: &crate::resources::ResourceBundle,
    text: &str,
) -> Vec<String> {
    let mut hits = Vec::new();
    for ke in &res.keyword_evidence {
        let matched = ke
            .keywords
            .iter()
            .any(|kw| text.contains(kw.as_str()));
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
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    ranked.into_iter().map(|(slug, _)| slug).collect()
}
