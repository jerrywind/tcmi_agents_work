//! 远端能力公告（与 rrserver/src/skill.rs 一致）。
//!
//! 用于家庭端上报自身 capability（如 "acme"，后接 JSON 负载），
//! 与治理相关的简单状态机 / 限流规则占位（阶段 A 原样复用）。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// 上报的状态（字符串标签）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConstState(pub String);

impl ConstState {
    pub fn new(s: &str) -> Self {
        ConstState(s.to_string())
    }
}

/// 状态判断引擎（阶段 A 仅支持 ConstState）。
pub struct JudgeEngine {
    budget: u64,
    _state: Arc<ConstState>,
}

impl JudgeEngine {
    pub fn new(budget: u64, state: Arc<ConstState>) -> Self {
        JudgeEngine { budget, _state: state }
    }
    pub fn can_afford(&self, cost: u64) -> bool {
        cost <= self.budget
    }
}

/// 单个能力规则。
#[derive(Debug, Clone)]
pub struct SkillRule {
    pub name: String,
    pub cooldown: Duration,
    pub cost: u64,
    pub required_state: Option<String>,
}

/// 能力集合 + 限流/状态机（占位实现）。
pub struct SkillSet {
    engine: Arc<JudgeEngine>,
    rules: Mutex<HashMap<String, SkillRule>>,
    last_used: Mutex<HashMap<String, Instant>>,
    used_total: AtomicU64,
}

impl SkillSet {
    pub fn new(engine: Arc<JudgeEngine>) -> Self {
        SkillSet {
            engine,
            rules: Mutex::new(HashMap::new()),
            last_used: Mutex::new(HashMap::new()),
            used_total: AtomicU64::new(0),
        }
    }
    pub fn register(&self, rule: SkillRule) {
        self.rules.lock().unwrap().insert(rule.name.clone(), rule);
    }
    /// 评估是否可触发某能力（含预算/冷却判断）。
    pub fn evaluate(&self, name: &str) -> bool {
        let rules = self.rules.lock().unwrap();
        let rule = match rules.get(name) {
            Some(r) => r.clone(),
            None => return false,
        };
        drop(rules);
        if !self.engine.can_afford(rule.cost) {
            return false;
        }
        let mut last = self.last_used.lock().unwrap();
        if let Some(t) = last.get(name) {
            if t.elapsed() < rule.cooldown {
                return false;
            }
        }
        last.insert(name.to_string(), Instant::now());
        self.used_total.fetch_add(1, Ordering::Relaxed);
        true
    }
}

/// 注册请求 / 响应（家庭端 announce 用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Announce {
    pub cap: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}
