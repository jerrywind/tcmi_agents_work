//! 用药安全校验：妊娠期禁忌 / 配伍禁忌（十八反十九畏）/ 毒性药
//!
//! 复刻 backend `app/knowledge/herb_safety.py:check_herb_safety`。
//! 规则数据放在 `resources/` 之外以常量固化（属医学硬约束，不建议随意改）；
//! 但「警示文案」可由 prompts/safety.yaml 提供。

use serde_json::{json, Value};

/// 十八反：每组中的药不能同用
const EIGHTEEN_INCOMPAT: &[&[&str]] = &[
    &["甘草", "海藻"], &["甘草", "大戟"], &["甘草", "芫花"], &["甘草", "甘遂"],
    &["乌头", "半夏"], &["乌头", "瓜蒌"], &["乌头", "贝母"], &["乌头", "白蔹"], &["乌头", "白及"],
    &["藜芦", "人参"], &["藜芦", "沙参"], &["藜芦", "丹参"], &["藜芦", "玄参"], &["藜芦", "细辛"], &["藜芦", "芍药"],
];

/// 十九畏：成对待忌
const NINETEEN_CONTRA: &[&[&str]] = &[
    &["硫黄", "朴硝"], &["水银", "砒霜"], &["狼毒", "密陀僧"],
    &["巴豆", "牵牛"], &["丁香", "郁金"], &["川乌", "犀角"],
    &["牙硝", "三棱"], &["官桂", "石脂"], &["人参", "五灵脂"],
];

/// 妊娠期禁用/慎用
const PREGNANCY_BAN: &[&str] = &[
    "附子", "肉桂", "干姜", "桃仁", "红花", "三棱", "莪术", "水蛭", "虻虫",
    "麝香", "巴豆", "大黄", "芒硝", "牛膝", "冬葵子", "瞿麦",
];

#[derive(Debug, Clone)]
pub struct HerbSafetyHit {
    pub kind: String,   // 冲突类型
    pub detail: String, // 详细说明
}

/// 检查一组药名是否触发安全规则。
/// `pregnant` 表示患者是否妊娠。返回命中列表。
pub fn check_herb_safety(herbs: &[String], pregnant: bool) -> Vec<HerbSafetyHit> {
    let mut hits = Vec::new();

    // 配伍禁忌
    for group in EIGHTEEN_INCOMPAT.iter().chain(NINETEEN_CONTRA.iter()) {
        let a = group[0];
        let b = group[1];
        if herbs.iter().any(|h| h.contains(a)) && herbs.iter().any(|h| h.contains(b)) {
            hits.push(HerbSafetyHit {
                kind: "incompatibility".into(),
                detail: format!("配伍禁忌：{a} 与 {b} 不宜同用"),
            });
        }
    }

    // 妊娠禁忌
    if pregnant {
        for h in herbs {
            if PREGNANCY_BAN.iter().any(|b| h.contains(*b)) {
                hits.push(HerbSafetyHit {
                    kind: "pregnancy".into(),
                    detail: format!("妊娠禁忌：{h} 为妊娠禁用/慎用之品"),
                });
            }
        }
    }

    hits
}

/// 把命中结果序列化为 JSON（供 agent 拼入文案）
pub fn hits_to_json(hits: &[HerbSafetyHit]) -> Value {
    let arr: Vec<Value> = hits
        .iter()
        .map(|h| json!({"kind": h.kind, "detail": h.detail}))
        .collect();
    json!(arr)
}
