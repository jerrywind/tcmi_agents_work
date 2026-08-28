//! PPG（体检报告）解析：把「数词 + 单位」文本翻译为临床数值
//!
//! 复刻 backend `app/knowledge/ppg.py:parse_ppg`。

use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;

/// 中文数词 -> 阿拉伯数字
fn cn_num(s: &str) -> Option<f64> {
    let map: HashMap<char, f64> = [
        ('零', 0.0), ('〇', 0.0), ('一', 1.0), ('二', 2.0), ('两', 2.0),
        ('三', 3.0), ('四', 4.0), ('五', 5.0), ('六', 6.0), ('七', 7.0),
        ('八', 8.0), ('九', 9.0), ('十', 10.0), ('百', 100.0), ('千', 1000.0),
    ]
    .iter()
    .cloned()
    .collect();
    if let Ok(v) = s.parse::<f64>() {
        return Some(v);
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let mut total = 0.0;
    let mut section = 0.0;
    for &c in &chars {
        if let Some(n) = map.get(&c) {
            if *n >= 10.0 {
                if section == 0.0 {
                    section = 1.0;
                }
                section *= n;
            } else {
                section += n;
            }
        }
    }
    total += section;
    if total == 0.0 {
        None
    } else {
        Some(total)
    }
}

/// 解析一段 PPG 文本，返回 {指标: 数值} 映射
pub fn parse_ppg(text: &str) -> Value {
    let mut out = serde_json::Map::new();

    let bp_re = Regex::new(r"(?i)(血压|收缩压|舒张压)[^\d一-龥]{0,6}(\d+|[一二两三四五六七八九十百零〇]+)\s*/\s*(\d+|[一二两三四五六七八九十百零〇]+)").unwrap();
    if let Some(cap) = bp_re.captures(text) {
        if let (Some(s), Some(d)) = (cn_num(&cap[2]), cn_num(&cap[3])) {
            out.insert("systolic".into(), json!(s));
            out.insert("diastolic".into(), json!(d));
        }
    }

    let temp_re = Regex::new(r"(?i)(体温|温度)[^\d.一-龥]{0,6}(\d+(?:\.\d+)?|三十六点五|三十七)").unwrap();
    if let Some(cap) = temp_re.captures(text) {
        if let Some(v) = cn_num(&cap[2]) {
            out.insert("temperature".into(), json!(v));
        } else if &cap[2] == "三十六点五" {
            out.insert("temperature".into(), json!(36.5));
        } else if &cap[2] == "三十七" {
            out.insert("temperature".into(), json!(37.0));
        }
    }

    let hr_re = Regex::new(r"(?i)(心率|脉搏|心律)[^\d一-龥]{0,6}(\d+|[一二两三四五六七八九十百零〇]+)\s*(?:次|下|bpm)?").unwrap();
    if let Some(cap) = hr_re.captures(text) {
        if let Some(v) = cn_num(&cap[2]) {
            out.insert("heart_rate".into(), json!(v));
        }
    }

    let glu_re = Regex::new(r"(?i)(血糖)[^\d.一-龥]{0,6}(\d+(?:\.\d+)?)").unwrap();
    if let Some(cap) = glu_re.captures(text) {
        if let Ok(v) = cap[2].parse::<f64>() {
            out.insert("glucose".into(), json!(v));
        }
    }

    json!(out)
}
