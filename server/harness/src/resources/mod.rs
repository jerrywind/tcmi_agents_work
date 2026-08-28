//! 可修改资源层：流程与数据分离
//!
//! 所有「可改文字 / 规则」都放在 `resources/*.yaml`，由中医专业人士维护；
//! 程序逻辑在 `agents/`、`knowledge/` 等模块，不直接写死文案。

pub mod bundle;
pub mod model;

pub use bundle::load;
pub use model::*;
