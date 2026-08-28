//! server 库：合并 backend（中医诊疗编排）与 rrserver（反向隧道中继）的统一实现。
//!
//! 顶层模块：
//! - `relay`   ：从 rrserver 迁入的中继/隧道能力（云端 server + 家庭端 client + llm_server 包装）。
//! - `diagnose`：中医诊断编排（阶段 A 为骨架，后续阶段落地 7 个 Sub-Agent / 报告 / 流式）。
//! - `models`  ：领域数据模型（阶段 A 为占位，与 backend/app/models/schemas.py 对齐）。
//! - `store`   ：会话/家庭持久化（阶段 A 为内存空壳）。
//! - `api`     ：HTTP 路由（阶段 A 含 /health、/api/health，后续补全 /api/consultations 等）。
//! - `config`  ：配置加载（合并 tunnels 与 diagnose settings）。

pub mod api;
pub mod config;
pub mod diagnose;
pub mod models;
pub mod relay;
pub mod store;
