//! HTTP 服务（axum）
//!
//! 复刻 backend `app/main.py` 的端点：
//! - POST /chat        完整诊断流程（orchestrator）
//! - POST /agents      单步调用某个 sub-agent
//! - GET  /agents      列出已注册 capability
//! - GET  /skills      列出可用技能
//! - POST /skills      执行某个技能
//! - GET  /health      健康检查
//! - POST /reload      热重载 YAML 资源（hot_reload 时）

use crate::model::{AgentRequest, Message};
use crate::AppState;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/agents", get(list_agents))
        .route("/chat", post(chat))
        .route("/agents", post(run_agent))
        .route("/skills", get(list_skills))
        .route("/skills", post(call_skill))
        .route("/reload", post(reload))
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

async fn list_agents(State(st): State<AppState>) -> Json<Value> {
    let caps = st.registry.capabilities();
    let names: Vec<String> = caps.iter().map(|c| c.zh().to_string()).collect();
    Json(json!({"capabilities": caps, "names": names}))
}

async fn chat(
    State(st): State<AppState>,
    Json(req): Json<Value>,
) -> Json<Value> {
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
        Ok(d) => Json(crate::orchestrator::diagnosis_payload(&d)),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

async fn run_agent(
    State(st): State<AppState>,
    Json(req): Json<AgentRequest>,
) -> Json<Value> {
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
        Ok((cap, text)) => Json(json!({
            "capability": cap,
            "content": text,
        })),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

async fn list_skills(State(st): State<AppState>) -> Json<Value> {
    let skills = st.skills.all();
    let list: Vec<Value> = skills
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "description": s.description,
                "owner": s.owner.map(|c| c.zh()).unwrap_or_else(|| "全局".into()),
            })
        })
        .collect();
    Json(json!({"skills": list}))
}

async fn call_skill(
    State(st): State<AppState>,
    Json(req): Json<Value>,
) -> Json<Value> {
    let name = req.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let args = req.get("arguments").cloned().unwrap_or(json!({}));
    let skills = st.skills.all();
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
