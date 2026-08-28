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
            })
        })
        .collect()
}

/// 按证候 slug 找调护方案
pub fn find_care(res: &ResourceBundle, syndrome_slug: &str) -> Vec<Value> {
    res.cares
        .iter()
        .filter(|c| c.for_syndromes.iter().any(|s| s == syndrome_slug))
        .map(|c| json!({"slug": c.slug, "label": c.label, "items": c.items}))
        .collect()
}
