//! sub-agent 基类：LLM 调用封装 + SubAgent trait
//!
//! 复刻 backend `app/protocol/llm.py` 的 `chat_completion` / `chat_with_tools`，
//! 对接 OpenAI 兼容网关（默认 lmstudio / llm_server）。

use crate::config::HarnessConfig;
use crate::model::{Capability, Message};
use crate::resources::ResourceBundle;
use crate::skills::{Skill, SkillRegistry};
use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};

/// agent 执行所需的上下文（持有 Arc，无生命周期约束，便于在异步 future 内使用）
#[derive(Clone)]
pub struct AgentContext {
    pub config: Arc<HarnessConfig>,
    pub resources: Arc<ResourceBundle>,
    pub llm: reqwest::Client,
    /// 全局技能注册表（供支持 tool calling 的 agent 使用）
    pub skills: Arc<SkillRegistry>,
}

/// sub-agent 统一接口
#[async_trait::async_trait]
pub trait SubAgent: Send + Sync {
    fn capability(&self) -> Capability;
    /// 执行一次请求，返回 AgentResponse
    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],
        payload: &Value,
    ) -> Result<String>;
}

/// 最简聊天补全（无工具）
pub async fn chat_completion(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    messages: &[Message],
) -> Result<String> {
    let mut body_msgs = Vec::new();
    if !system.is_empty() {
        body_msgs.push(json!({"role": "system", "content": system}));
    }
    for m in messages {
        body_msgs.push(json!({"role": m.role, "content": m.content}));
    }

    let resp = client
        .post(format!("{base_url}/chat/completions"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&json!({
            "model": model,
            "messages": body_msgs,
            "temperature": 0.3,
            "stream": false,
        }))
        .send()
        .await?
        .error_for_status()?;

    let v: Value = resp.json().await?;
    let content = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    Ok(content)
}

/// 带工具的聊天补全（tool calling），执行后返回模型最终文本
///
/// `capability` 用于从技能注册表中筛选「仅该 agent 可用」的工具；
/// 若该 agent 没有专属工具，则不启用 tool calling。
pub async fn chat_with_tools(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    messages: &[Message],
    skills: &SkillRegistry,
    capability: Capability,
) -> Result<String> {
    // 仅暴露该 capability 专属、或全局（无 owner）的技能
    let tools: Vec<Skill> = skills
        .for_capability(capability)
        .into_iter()
        .cloned()
        .collect();
    if tools.is_empty() {
        // 无可用工具时退化为普通补全
        return chat_completion(client, base_url, api_key, model, system, messages).await;
    }
    let tool_defs: Vec<Value> = tools
        .iter()
        .map(|s| {
            json!({
                "type": "function",
                "function": {
                    "name": s.name,
                    "description": s.description,
                    "parameters": s.parameters,
                }
            })
        })
        .collect();

    let mut body_msgs = Vec::new();
    if !system.is_empty() {
        body_msgs.push(json!({"role": "system", "content": system}));
    }
    for m in messages {
        body_msgs.push(json!({"role": m.role, "content": m.content}));
    }

    let resp = client
        .post(format!("{base_url}/chat/completions"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&json!({
            "model": model,
            "messages": body_msgs,
            "tools": tool_defs,
            "tool_choice": "auto",
            "temperature": 0.3,
            "stream": false,
        }))
        .send()
        .await?
        .error_for_status()?;

    let v: Value = resp.json().await?;
    let msg = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .cloned()
        .unwrap_or(json!({}));

    // 若模型请求调用工具，则执行并把结果回灌（单轮）
    if let Some(tool_calls) = msg.get("tool_calls").and_then(|t| t.as_array()) {
        let mut follow = body_msgs.clone();
        follow.push(msg.clone());
        for tc in tool_calls {
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("");
            let args: Value = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|a| serde_json::from_str::<Value>(a.as_str().unwrap_or("{}")).ok())
                .unwrap_or(json!({}));
            let id = tc.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
            let result: serde_json::Value = match crate::skills::dispatch(&tools, name, &args).await {
                Ok(v) => v,
                Err(e) => json!({"error": e.to_string()}),
            };
            follow.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": result.to_string(),
            }));
        }
        // 第二轮：汇总成最终回答
        let resp2 = client
            .post(format!("{base_url}/chat/completions"))
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&json!({
                "model": model,
                "messages": follow,
                "temperature": 0.3,
                "stream": false,
            }))
            .send()
            .await?
            .error_for_status()?;
        let v2: Value = resp2.json().await?;
        return Ok(v2
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string());
    }

    Ok(msg
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string())
}
