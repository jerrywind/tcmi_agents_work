//! 领域知识层：PPG 解析 / 用药安全 / 方剂检索
//!
//! 复刻 backend `app/knowledge/` 的纯函数逻辑（不依赖 LLM）。

pub mod herb_safety;
pub mod ppg;
pub mod treatments;

pub use herb_safety::{check_herb_safety, HerbSafetyHit};
pub use ppg::parse_ppg;
pub use treatments::{check_composition, find_care, find_formula};
