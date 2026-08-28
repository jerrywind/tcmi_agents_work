//! 会话 / 家庭持久化（阶段 A 内存空壳）。
//!
//! 后续阶段实现 `ConsultationStore` / `FamilyStore`，并提供 `RedisStore`（feature gate）。

use std::collections::HashMap;

use tokio::sync::RwLock;

/// 阶段 A：仅占位，后续承载 `Consultation` / `Family` 的内存映射。
#[derive(Default)]
pub struct MemoryStore {
    inner: RwLock<HashMap<String, String>>,
}
