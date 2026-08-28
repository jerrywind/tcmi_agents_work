//! 全局配置：合并 rrserver 的隧道配置与后续 backend 诊疗配置。
//!
//! 阶段 A 仅实现 tunnels / external_ws_base / skills 解析（原 rrserver 逻辑），
//! diagnose 相关字段在后续阶段补充。

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use toml::Value;

use crate::relay::skill::{ConstState, JudgeEngine, SkillRule, SkillSet};
use crate::relay::state::Registry;
use crate::relay::server::{AppState, TunnelAuth};

/// 解析 TOML 配置文件，构造云端中继所需的 `AppState`。
/// 阶段 A：等价于原 rrserver 的 `load_config` + `Server` 分支装配。
pub fn build_app_state(path: Option<&str>) -> anyhow::Result<AppState> {
    let (tokens, ws_base, skills) = load_config(path)?;
    if tokens.is_empty() {
        anyhow::bail!("no tunnels configured; add [[tunnels]] to config");
    }
    let auth = TunnelAuth::from_list(&tokens);
    let state = AppState {
        registry: Registry::new(),
        auth,
        external_ws_base: ws_base,
        skills,
        coordinator: crate::relay::state::ForwardCoordinator::new(),
    };
    Ok(state)
}

/// 加载 [[tunnels]]、external_ws_base、可选 [[skills]]。
pub fn load_config(
    path: Option<&str>,
) -> anyhow::Result<(Vec<(String, String)>, String, Option<Arc<SkillSet>>)> {
    let path = match path {
        Some(p) => p,
        None => return Ok((vec![], String::new(), None)),
    };
    let content = std::fs::read_to_string(path).with_context(|| format!("read config {}", path))?;
    let doc: Value = toml::from_str(&content).context("parse config")?;
    let mut tokens = vec![];
    if let Some(arr) = doc.get("tunnels").and_then(|v: &Value| v.as_array()) {
        for t in arr {
            let name = t
                .get("name")
                .and_then(|v: &Value| v.as_str())
                .unwrap_or("")
                .to_string();
            let token = t
                .get("token")
                .and_then(|v: &Value| v.as_str())
                .unwrap_or("")
                .to_string();
            if !name.is_empty() {
                tokens.push((name, token));
            }
        }
    }
    let ws_base = doc
        .get("external_ws_base")
        .and_then(|v: &Value| v.as_str())
        .unwrap_or("")
        .to_string();

    let skills = doc
        .get("skills")
        .and_then(|v: &Value| v.as_array())
        .filter(|a| !a.is_empty())
        .map(|arr| {
            let budget = doc
                .get("skill_budget")
                .and_then(|v: &Value| v.as_integer())
                .unwrap_or(1_000_000) as u64;
            let engine = Arc::new(JudgeEngine::new(budget, Arc::new(ConstState("idle".into()))));
            let set = Arc::new(SkillSet::new(engine));
            for s in arr {
                let name = s
                    .get("name")
                    .and_then(|v: &Value| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                let cooldown_secs = s
                    .get("cooldown_secs")
                    .and_then(|v: &Value| {
                        v.as_float()
                            .or_else(|| v.as_integer().map(|i| i as f64))
                    })
                    .unwrap_or(0.0);
                let cost = s
                    .get("cost")
                    .and_then(|v: &Value| v.as_integer())
                    .unwrap_or(0) as u64;
                let required_state = s
                    .get("required_state")
                    .and_then(|v: &Value| v.as_str())
                    .map(|s| s.to_string());
                set.register(SkillRule {
                    name,
                    cooldown: Duration::from_secs_f64(cooldown_secs.max(0.0)),
                    cost,
                    required_state,
                });
            }
            set
        });

    Ok((tokens, ws_base, skills))
}
