//! 方剂 / 调护检索
//!
//! 复刻 backend `app/knowledge/treatments.py` 的检索逻辑，
//! 但数据来自 `resources/formulas.yaml` 与 `resources/care.yaml`（可改）。

use crate::resources::ResourceBundle;
use serde_json::{json, Value};

/// 按证候 slug 找适用方剂
pub fn find_formula(res: &ResourceBundle, syndrome_slug: &str) -> Vec<Value> {
    res.formulas
        .iter()
        .filter(|f| f.for_syndromes.iter().any(|s| s == syndrome_slug))
        .map(|f| {
            json!({
                "slug": f.slug,
                "name": f.name,
                "composition": f.composition,
                "usage": f.usage,
                "caution": f.caution,
                "source": f.source,
            })
        })
        .collect()
}

/// 认定为「确实在列举药味」所需的最少命中味数
///
/// 判据不能是「命中 ≥1 味」：方名本身常含药名（「麻黄汤」含「麻黄」、
/// 「桂枝汤」含「桂枝」），一句「可考虑麻黄汤加减」就会命中 1 味，
/// 被误当成在列药味而报一堆「漏味」。取 2 味方足以区分这两种情形。
const MIN_HERBS_FOR_COMPOSITION_CHECK: usize = 2;

/// 药味一致性校验（T7.4）：拿文本对照方剂库记载的组成
///
/// 开方步的输出是自由文本，模型「记得」的经方组成未必对——
/// 人工验收出现过「麻黄汤：麻黄、桂枝、杏仁、白术、甘草」，原方并无白术。
/// 纯靠提示词要求「照抄方剂库」压不住这种幻觉，故再做一道确定性比对。
///
/// 触发条件刻意收紧：文本必须**既提到方名、又确实在列药味**
/// （至少命中组成中的 2 味），否则只是顺口提及方名，不该报。
/// 措辞是「请核对」而非断言错误——药名存在别名与炮制写法
/// （「甘草」/「炙甘草」），宁可漏报也不要把正确的输出标红。
pub fn check_composition(res: &ResourceBundle, text: &str) -> Vec<String> {
    let mut notes = Vec::new();
    for f in &res.formulas {
        if f.composition.len() < 2 {
            continue;
        }
        if !text.contains(f.name.as_str()) {
            continue;
        }
        let hit = f
            .composition
            .iter()
            .filter(|h| text.contains(h.as_str()))
            .count();
        if hit < MIN_HERBS_FOR_COMPOSITION_CHECK {
            continue;
        }
        let missing: Vec<&str> = f
            .composition
            .iter()
            .filter(|h| !text.contains(h.as_str()))
            .map(|s| s.as_str())
            .collect();
        if missing.is_empty() {
            continue;
        }
        notes.push(format!(
            "{}：方剂库记载组成为「{}」，本次输出中未见「{}」，请核对是否漏味。",
            f.name,
            f.composition.join("、"),
            missing.join("、")
        ));
    }
    notes
}

/// 按证候 slug 找调护方案
pub fn find_care(res: &ResourceBundle, syndrome_slug: &str) -> Vec<Value> {
    res.cares
        .iter()
        .filter(|c| c.for_syndromes.iter().any(|s| s == syndrome_slug))
        .map(|c| json!({"slug": c.slug, "label": c.label, "items": c.items}))
        .collect()
}
