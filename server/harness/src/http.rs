//! HTTP 服务（axum）
//!
//! 复刻 backend `app/main.py` 的端点：
//! - POST /chat        完整诊断流程（orchestrator）
//! - POST /agents      单步调用某个 sub-agent
//! - GET  /agents      列出已注册 capability
//! - GET  /skills      列出可用技能
//! - POST /skills      执行某个技能
//! - POST /mcp         MCP Server 端点（T4.5，对外暴露 7 个能力）
//! - GET  /health      健康检查
//! - POST /reload      热重载 YAML 资源（hot_reload 时）

use crate::model::{AgentRequest, Capability, Message};
use crate::skills::Skill;
use crate::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/agents", get(list_agents))
        .route("/chat", post(chat))
        .route("/agents", post(run_agent))
        .route("/skills", get(list_skills))
        .route("/skills", post(call_skill))
        .route("/mcp", post(mcp))
        .route("/reload", post(reload))
        .with_state(state)
}

/// MCP Server 端点（T4.5）
///
/// 通知类请求（无 `id`）按协议不回包，返回 204 No Content。
async fn mcp(State(st): State<AppState>, Json(req): Json<Value>) -> Response {
    match crate::mcp::server::handle(&st, &req).await {
        Some(v) => Json(v).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

async fn health() -> &'static str {
    "ok"
}

async fn list_agents(State(st): State<AppState>) -> Json<Value> {
    let caps = st.registry.capabilities();
    let names: Vec<String> = caps.iter().map(|c| c.zh().to_string()).collect();
    Json(json!({"capabilities": caps, "names": names}))
}

async fn chat(State(st): State<AppState>, Json(req): Json<Value>) -> Json<Value> {
    let messages: Vec<Message> = req
        .get("messages")
        .and_then(|m| serde_json::from_value(m.clone()).ok())
        .unwrap_or_default();
    let payload = req.get("payload").cloned().unwrap_or(json!({}));

    let res = st.resources.read().await;
    match crate::orchestrator::run_diagnosis(
        &st.registry,
        &st.config,
        &res,
        &st.llm,
        &st.skills,
        &messages,
        &payload,
    )
    .await
    {
        Ok(d) => {
            // 全部步骤都失败时才算整体失败（多半是 LLM 不可达）。
            // 只要有一步成功就按部分成功返回，避免前面的计算全部作废。
            if d.steps.is_empty() && !d.failures.is_empty() {
                let first = d.failures[0].1.clone();
                Json(json!({"error": first, "failures": d.failures.iter()
                    .map(|(c, e)| json!({"capability": c, "error": e}))
                    .collect::<Vec<_>>()}))
            } else {
                Json(crate::orchestrator::diagnosis_payload(&d))
            }
        }
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

async fn run_agent(State(st): State<AppState>, Json(req): Json<AgentRequest>) -> Json<Value> {
    let res = st.resources.read().await;
    match crate::orchestrator::run_single(
        &st.registry,
        &st.config,
        &res,
        &st.llm,
        &st.skills,
        req.capability,
        &req.messages,
        &req.payload,
    )
    .await
    {
        Ok((cap, text, trace, structured)) => Json(json!({
            "capability": cap,
            "content": text,
            "trace": trace,
            // 结构化输出（T4.1）：无结构化结果的步骤为 null，字段恒定存在
            "structured": structured.unwrap_or(serde_json::Value::Null),
        })),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

/// 取某 capability 可用的技能（专属 + 全局）；`None` 表示不过滤
fn skills_for(st: &AppState, owner: Option<&str>) -> Option<Vec<Skill>> {
    let owner = owner?;
    let cap = Capability::from_name(owner)?;
    Some(st.skills.for_capability(cap).into_iter().cloned().collect())
}

async fn list_skills(
    State(st): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<Value> {
    // 可选 ?owner=<slug 或中文名>：只看某 capability 能用的工具
    let skills = match q.get("owner").map(|s| s.as_str()) {
        Some(o) => match skills_for(&st, Some(o)) {
            Some(list) => list,
            None => {
                return Json(
                    json!({"error": format!("未知 owner: {o}（用 /agents 查看可用 capability）")}),
                )
            }
        },
        None => st.skills.all(),
    };
    let list: Vec<Value> = skills
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "description": s.description,
                "owner": s.owner.map(|c| c.zh()).unwrap_or("全局"),
            })
        })
        .collect();
    Json(json!({"skills": list}))
}

async fn call_skill(State(st): State<AppState>, Json(req): Json<Value>) -> Json<Value> {
    let name = req.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let args = req.get("arguments").cloned().unwrap_or(json!({}));
    // 可选 owner 过滤（T2.5）：把可调用范围限制为「该 capability 用得到的技能」，
    // 即其专属技能 + 全局技能。此前完全不过滤，专属技能可被任意调用方执行。
    let skills = match req.get("owner").and_then(|v| v.as_str()) {
        Some(o) => match skills_for(&st, Some(o)) {
            Some(list) => list,
            None => return Json(json!({"error": format!("未知 owner: {o}")})),
        },
        None => st.skills.all(),
    };
    match crate::skills::dispatch(&skills, name, &args).await {
        Ok(r) => Json(json!({"result": r})),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

async fn reload(State(st): State<AppState>) -> Json<Value> {
    match st.reload_resources().await {
        Ok(_) => Json(json!({"ok": true})),
        Err(e) => Json(json!({"ok": false, "error": e.to_string()})),
    }
}
