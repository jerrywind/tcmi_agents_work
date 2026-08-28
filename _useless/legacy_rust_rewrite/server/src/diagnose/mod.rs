//! 中医诊断编排模块（阶段 A 骨架）。
//!
//! 后续阶段将在此落地：
//! - `orchestrator`：诊断 Loop（start/start_sync/answer/rounds/report）
//! - `agents`：7 个 Sub-Agent（望闻问切 + 辨证 + 知识 + 安全）
//! - `knowledge`：证候/问卷/建议/用药禁忌库
//! - `report`：报告/方案/待办/随访生成
//! - `stream`：流式分段
//! - `ppg`：脉象解析
//!
//! 阶段 A 仅提供 `api_router()` 空壳，挂载到 `/api` 下，后续逐步补全。

use axum::Router;

/// 诊断模块路由空壳。后续阶段在此 `.merge(consultations::router())` 等。
pub fn api_router() -> Router {
    Router::new()
}
