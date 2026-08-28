//! HTTP 路由层。
//!
//! 阶段 A：仅实现 `/health`、`/api/health`、以及把中继能力（`relay::server::build_router`）
//! 与诊断空壳路由合并挂载。后续阶段在 `consultations.rs` / `families.rs` 等补全
//! backend 的 `/api/consultations`、`/api/families` 等契约。

use axum::{
    routing::{get, Router},
    Json,
};

use crate::config::build_app_state;
use crate::relay::server::AppState;

/// 构造顶层路由：
/// - `/health` `/healthz`：探活（保留 rrserver 的 /healthz 兼容性）
/// - `/api/health`：诊断服务探活
/// - 中继路由（注册 / WS / `/t/:name/*`）由 `relay::server::build_router` 提供，合并进来
pub fn build_router(state: AppState) -> Router {
    let relay_router = crate::relay::server::build_router(state);

    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/health", get(api_health))
        // 诊断模块路由（阶段 A 空壳，后续挂载 consultations/families 等）
        .merge(crate::diagnose::api_router())
        // 中继路由（含 /healthz、/api/register、/ws/:name、/t/:name/*）
        .merge(relay_router)
}

async fn api_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true, "service": "diagnose", "stage": "A" }))
}

/// 便捷构造：从配置文件直接构建状态并生成路由。
pub fn build_router_from_config(path: Option<&str>) -> anyhow::Result<Router> {
    let state = build_app_state(path)?;
    Ok(build_router(state))
}
