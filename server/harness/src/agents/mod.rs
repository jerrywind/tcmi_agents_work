//! sub-agent 注册表与调度
//!
//! 十三个 sub-agent 对应 13 个 Capability，按问诊流程分四期：
//!
//! - **采集**：inspection（望诊）/ listening（闻诊）/ inquiry（问诊）/ palpation（切诊）
//! - **辨证**：case_reference（医案参考）/ differentiation（辨证）
//! - **安全**：safety（安全门）
//! - **治疗**：strategy（立法）/ herbology（用药）/ prescription（开方）/
//!   care（调护）/ acupuncture（针灸）
//! - **兼容**：treatment（综合治疗，旧流程的一步到位版本）
//!
//! 每个 agent 实现 `SubAgent` trait，由 `Registry` 按 capability 名查找并分发。
//! 各 agent 的典籍检索域见 `resources/rag_scopes.yaml`。

pub mod acupuncture;
pub mod base;
pub mod care;
pub mod case_reference;
pub mod convergence;
pub mod differentiation;
pub mod herbology;
pub mod inquiry;
pub mod inspection;
pub mod listening;
pub mod palpation;
pub mod prescription;
pub mod safety;
pub mod strategy;
pub mod treatment;

pub use base::{AgentContext, LlmCaller, SubAgent};

use crate::model::Capability;
use crate::resources::model::RedFlag;
use crate::resources::ResourceBundle;
use std::collections::HashMap;
use std::sync::Arc;

/// 规范顺序：望 → 闻 → 问 → 切 → 医案 → 辨证 → 安全门 → 立法 → 用药 → 开方 → 调护 → 针灸 → 治疗
///
/// `Registry` 内部用 HashMap 存储，**迭代顺序不稳定**（Rust 的 HashMap
/// 使用随机化哈希，每次进程启动顺序都可能不同）。对外暴露能力清单时必须
/// 按此顺序，否则 `GET /agents` 每次重启返回的顺序都会变，
/// 前端分步展示与契约测试都会被随机顺序打乱。
///
/// 写成 `Capability::ALL` 的别名而非重复列举：新增能力时只需改枚举一处。
const CAPABILITY_ORDER: [Capability; 13] = Capability::ALL;

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
            Capability::CaseReference,
            Arc::new(case_reference::CaseReferenceAgent),
        );
        map.insert(
            Capability::Differentiation,
            Arc::new(differentiation::DifferentiationAgent),
        );
        map.insert(Capability::Safety, Arc::new(safety::SafetyAgent));
        // ---- 治疗期：由旧的一步到位拆成「立法 → 用药 → 开方」，调护/针灸可选 ----
        map.insert(Capability::Strategy, Arc::new(strategy::StrategyAgent));
        map.insert(Capability::Herbology, Arc::new(herbology::HerbologyAgent));
        map.insert(
            Capability::Prescription,
            Arc::new(prescription::PrescriptionAgent),
        );
        map.insert(Capability::Care, Arc::new(care::CareAgent));
        map.insert(
            Capability::Acupuncture,
            Arc::new(acupuncture::AcupunctureAgent),
        );
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

/// 把「证候 slug / 中文名」归一化成证候库里的 slug。
///
/// 调用方（含前端）既可能传 `spleen_stomach_damp_heat` 也可能传「脾胃湿热」，
/// 而 `find_formula` / `find_care` 只按 slug 匹配，传中文名会静默查不到任何方剂。
/// 故在此统一归一：先按 slug 找，再按中文名找，都找不到才返回 `None`。
pub fn normalize_syndrome_slug(res: &ResourceBundle, raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if res.syndrome(raw).is_some() {
        return Some(raw.to_string());
    }
    res.syndromes
        .iter()
        .find(|s| s.name == raw)
        .map(|s| s.slug.clone())
}

/// 解析本步应依据的证候 slug —— **治疗期各步取证候的唯一入口**。
///
/// 此前每个治疗期 agent 各自写一段
/// `payload["syndrome"].or_else(|| infer_syndrome_slug(...).next())`：
/// `infer_syndrome_slug` 是关键词计数，与辨证步的 `assess()`
/// （置信度 / 支持证据 / 矛盾证据 / 兼证）根本不是同一套算法，也没有阈值。
/// 于是同一份语料里，辨证步判「脾胃湿热」、开方步可能按「肝胆湿热」下药——
/// 这正是人工验收里「方剂与主证不对口」「前后不一致」的直接来源。
///
/// 现在统一走这里，优先级：
/// 1. `payload.syndrome`（调用方显式给定，或编排器辨证后注入的**权威主证**）；
/// 2. 都没有时才退回文本推断（单步直接调用 agent 的场景，如 `POST /agents`）。
///
/// `payload.syndrome` 给了证候库里不存在的取值时**不静默吞掉**：
/// 记一条 WARN 后退回文本推断，否则写错一次就永远拿不到方剂。
pub fn resolve_syndrome(
    res: &ResourceBundle,
    messages: &[crate::model::Message],
    payload: &serde_json::Value,
) -> Option<String> {
    // H3 配套：辨证环节已判定「未匹配到证候」时**不再兜底猜测**。
    //
    // 兜底推断是给「单步直接调用 agent、调用方没给证候」准备的。
    // 而在完整流程里，辨证步既然说了「定不了证」，治疗期再猜一个出来，
    // 规则层就会把「【证候】气阴两虚证 /【治则】益气养阴」拼进输出——
    // 真实验证里正是这样：正文开头写着「本次未辨出明确证候」，
    // 末尾却摆着一个具体的证与治则，自相矛盾，
    // 后续的用药、开方还跟着这个猜错的证一路跑偏。
    if payload.get("syndrome_matched").and_then(|v| v.as_bool()) == Some(false) {
        tracing::info!("辨证未匹配到证候，治疗期各步不再兜底推断证候");
        return None;
    }
    if let Some(raw) = payload.get("syndrome").and_then(|v| v.as_str()) {
        match normalize_syndrome_slug(res, raw) {
            Some(slug) => return Some(slug),
            None => {
                if !raw.trim().is_empty() {
                    tracing::warn!(
                        syndrome = %raw,
                        "payload.syndrome 不在证候库中，回退到文本推断"
                    );
                }
            }
        }
    }
    infer_syndrome_slug(res, messages).into_iter().next()
}

/// 从对话文本推断相关证候 slug 列表（降序，按关键词命中数计分）。
///
/// 通过 `keywords.yaml` 的证据 -> 证候映射统计得分，返回**所有得分 > 0**
/// 的证候 slug（降序）。保留多个候选以支持「兼证」场景，调用方可按需取
/// 首位或整体参与辨证。
///
/// ⚠️ 这是**兜底推断**，不是权威结论：治疗期各步应改用 [`resolve_syndrome`]，
/// 以辨证步的主证为准。
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
    // 得分降序；**同分按证候库顺序**。
    //
    // 只按得分排序时，同分项的先后取决于 HashMap 的迭代顺序——
    // 那在每个进程里都可能不同（T3.7 已在这上面栽过一次：
    // `GET /agents` / `GET /skills` 返回的清单顺序随机）。
    // 兜底推断本来就是「说不准」的，再叠一层随机会让问题更难复现：
    // 同一个输入这次退到 A、下次退到 B，报告结论跟着漂。
    let order: std::collections::HashMap<&str, usize> = res
        .syndromes
        .iter()
        .enumerate()
        .map(|(i, s)| (s.slug.as_str(), i))
        .collect();
    ranked.sort_by_key(|(slug, n)| {
        (
            std::cmp::Reverse(*n),
            order.get(slug.as_str()).copied().unwrap_or(usize::MAX),
        )
    });
    ranked.into_iter().map(|(slug, _)| slug).collect()
}

/// 证候不确定提示（H6）：拼在 system 末尾，治疗期三步共用。
///
/// 背景：`lock_syndrome` 在置信度不足时**不锁定**证候（H4），
/// 治疗期各步因此退回文本推断——但模型不知道这件事，它会照样
/// 一板一眼地立法、用药、开方。规则层已经说了「不可信」，
/// 生成时却没告诉模型，等于没说。
///
/// 只在编排器**明确标注**了不确定时才返回非空：
/// 未匹配（`syndrome_matched=false`）或置信度不足（`syndrome_locked=false`）。
/// 单步调用（payload 里没有这些字段）返回空串，不改变既有行为。
pub fn syndrome_uncertainty_note(payload: &serde_json::Value) -> String {
    let name = || {
        payload
            .get("syndrome_name")
            .and_then(|v| v.as_str())
            .unwrap_or("（未定）")
    };
    let conf = || {
        payload
            .get("syndrome_confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    };

    if payload.get("syndrome_matched").and_then(|v| v.as_bool()) == Some(false) {
        return "\n\n【重要：本次未定证】\n\
             辨证环节**没有匹配到任何明确证候**：四诊信息未满足库内任一证候的主症必备条件。\n\
             请务必：\n\
             1. 在本节开头写明「本次未辨出明确证候，以下内容为一般性参考，非辨证结论」；\n\
             2. **不得**给出确定性的方剂、药物与剂量——证候未定则不立方；\n\
             3. 给出建议进一步采集哪些信息（舌象、寒热、二便等；脉象不单独采集，由系统据他证推断），并建议线下就诊。\n\
             宁可少说，也不要把猜测包装成结论。"
            .to_string();
    }

    if payload.get("syndrome_locked").and_then(|v| v.as_bool()) == Some(false) {
        return format!(
            "\n\n【重要：证候为低置信度推断】\n\
             本次辨证的主证「{}」置信度仅 {:.2}，未达锁定门槛 {:.2}，\
             属于**未经核实的推断**，不是确定结论。\n\
             请务必：\n\
             1. 在本节开头写明「证候为低置信度推断，需线下就诊确认」；\n\
             2. 不要给出确定性的方剂与剂量，只给方向性建议；\n\
             3. 不要把推断说成定论——读报告的人没有能力分辨这是猜的。",
            name(),
            conf(),
            crate::orchestrator::LOCK_MIN_CONFIDENCE
        );
    }

    String::new()
}
