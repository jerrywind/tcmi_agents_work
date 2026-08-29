//! 核心数据模型
//!
//! 仅保留**实际被使用**的类型。原 backend 的 `AgentResponse` / `SkillCall` /
//! `SkillResult` / `RequestFrame` / `ResponseFrame` 是从 Python schema 平移过来的
//! 残留，在 Rust 侧没有任何引用点（`SubAgent::run` 直接返回 `String`，
//! HTTP 层直接用 `serde_json::Value`，rrserver 有自己的 `protocol.rs`），已删除。

use serde::{Deserialize, Serialize};

/// 一次问诊/对话消息（用户或模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String, // "user" | "assistant" | "system"
    pub content: String,
}

/// 可调用能力（即 sub-agent 名）。对应 backend 的 `Capability` 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Inspection,      // 望诊
    Listening,       // 闻诊
    Inquiry,         // 问诊
    Palpation,       // 切诊
    Differentiation, // 辨证
    Safety,          // 安全门
    Treatment,       // 治疗
}

impl Capability {
    /// 全部能力，**按规范顺序**（望 → 闻 → 问 → 切 → 辨证 → 安全门 → 治疗）
    ///
    /// 清单对外暴露时必须按此顺序，否则依赖 HashMap 迭代顺序会随机变化。
    pub const ALL: [Capability; 7] = [
        Capability::Inspection,
        Capability::Listening,
        Capability::Inquiry,
        Capability::Palpation,
        Capability::Differentiation,
        Capability::Safety,
        Capability::Treatment,
    ];

    /// 中文名（用于日志与展示，不进入协议键）
    pub fn zh(&self) -> &'static str {
        match self {
            Capability::Inspection => "望诊",
            Capability::Listening => "闻诊",
            Capability::Inquiry => "问诊",
            Capability::Palpation => "切诊",
            Capability::Differentiation => "辨证",
            Capability::Safety => "安全门",
            Capability::Treatment => "治疗",
        }
    }

    /// 英文 slug（协议字段、MCP 工具名、YAML 路由表都用它）
    pub fn slug(&self) -> &'static str {
        match self {
            Capability::Inspection => "inspection",
            Capability::Listening => "listening",
            Capability::Inquiry => "inquiry",
            Capability::Palpation => "palpation",
            Capability::Differentiation => "differentiation",
            Capability::Safety => "safety",
            Capability::Treatment => "treatment",
        }
    }

    /// 从英文 slug 解析
    ///
    /// 以 [`Self::slug`] 为唯一映射来源，避免两处各写一份 match 后走样。
    pub fn from_slug(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.slug() == s)
    }

    /// 从 slug **或中文名**解析。
    ///
    /// `POST /skills` 的 `owner` 过滤两种写法都接受（前端拿得到中文名，
    /// YAML/日志里则是 slug），故这里同时支持。
    pub fn from_name(s: &str) -> Option<Self> {
        Self::from_slug(s).or_else(|| Self::ALL.iter().copied().find(|c| c.zh() == s))
    }
}

/// 后端统一的请求体（前端 / rrserver 透传）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    pub capability: Capability,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default)]
    pub payload: serde_json::Value,
}
