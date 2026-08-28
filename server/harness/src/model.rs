//! 核心数据模型
//!
//! 对应原 backend `app/models/schemas.py` 中的 Pydantic 模型。
//! 这里用 Rust 结构体复刻：AgentRequest / AgentResponse / SkillCall / SkillResult
//! 等，全部可序列化为 JSON，与 rrserver 的协议帧、前端 API 共用。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 一次问诊/对话消息（用户或模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,        // "user" | "assistant" | "system"
    pub content: String,
}

/// 可调用能力（即 sub-agent 名）。对应 backend 的 `Capability` 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Inspection,   // 望诊
    Listening,    // 闻诊
    Inquiry,      // 问诊
    Palpation,    // 切诊
    Differentiation, // 辨证
    Safety,       // 安全门
    Treatment,    // 治疗
}

impl Capability {
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

    /// 从英文 slug 解析（YAML 路由表用）
    pub fn from_slug(s: &str) -> Option<Self> {
        match s {
            "inspection" => Some(Capability::Inspection),
            "listening" => Some(Capability::Listening),
            "inquiry" => Some(Capability::Inquiry),
            "palpation" => Some(Capability::Palpation),
            "differentiation" => Some(Capability::Differentiation),
            "safety" => Some(Capability::Safety),
            "treatment" => Some(Capability::Treatment),
            _ => None,
        }
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

/// 后端统一的响应体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    pub capability: Capability,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// 工具/技能调用（sub-agent 向 LLM 请求的 tool call）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: HashMap<String, serde_json::Value>,
}

/// 工具/技能执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillResult {
    pub id: String,
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub result: serde_json::Value,
    #[serde(default)]
    pub error: Option<String>,
}

/// rrserver 协议帧：请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestFrame {
    pub id: String,
    pub capability: Capability,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// rrserver 协议帧：响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseFrame {
    pub id: String,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(default)]
    pub error: Option<String>,
}
