//! sub-agent 基类：LLM 调用封装 + SubAgent trait
//!
//! 对接 OpenAI 兼容网关（默认 lmstudio / llm_server）。
//! 三个横切关注点集中在 `LlmCaller`，避免散落到各 agent：
//! - **多轮工具调用**（T2.2）：模型可在工具结果之上继续查证，`max_tool_rounds` 可配；
//! - **失败重试**（T3.5）：超时 / 连接失败 / 5xx / 429 才重试，4xx 语义错误直接失败；
//! - **埋点**（T3.1）：耗时、重试次数、token、工具调用名与错误写入 `TraceHandle`。

use crate::config::HarnessConfig;
use crate::model::{Capability, Message};
use crate::resources::ResourceBundle;
use crate::skills::{Skill, SkillRegistry};
use crate::trace::{self, LlmCallStat, TraceHandle};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{json, Value};

/// agent 执行所需的上下文（持有 Arc，无生命周期约束，便于在异步 future 内使用）
#[derive(Clone)]
pub struct AgentContext {
    pub config: Arc<HarnessConfig>,
    pub resources: Arc<ResourceBundle>,
    pub llm: reqwest::Client,
    /// 全局技能注册表（供支持 tool calling 的 agent 使用）
    pub skills: Arc<SkillRegistry>,
    /// 本步骤的埋点累加器（耗时 / token / 工具调用 / 错误）
    pub trace: TraceHandle,
}

impl AgentContext {
    /// 构造上下文（埋点累加器随之初始化）
    pub fn new(
        config: Arc<HarnessConfig>,
        resources: Arc<ResourceBundle>,
        llm: reqwest::Client,
        skills: Arc<SkillRegistry>,
    ) -> Self {
        Self {
            config,
            resources,
            llm,
            skills,
            trace: trace::new_trace(),
        }
    }

    /// 取得绑定本上下文的 LLM 调用器（已带上配置、技能与埋点句柄）
    pub fn caller(&self) -> LlmCaller<'_> {
        LlmCaller {
            client: &self.llm,
            base_url: &self.config.llm_base_url,
            api_key: &self.config.llm_api_key,
            model: &self.config.model,
            max_retries: self.config.llm_max_retries,
            backoff_ms: self.config.llm_retry_backoff_ms,
            max_tool_rounds: self.config.max_tool_rounds,
            skills: Some(&self.skills),
            trace: Some(&self.trace),
        }
    }
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

    /// 可选的结构化输出（T4.1）：随响应原样返回给调用方，默认没有。
    ///
    /// 正文 `run` 返回的是给**人**看的 Markdown，前端只能整段渲染；
    /// 有些结论（证候 / 置信度 / 证据链）需要被**程序**消费——
    /// 卡片化展示、兼证并列、后续检索都要用到具体字段，
    /// 从 Markdown 里反解析既脆又易碎。故另开一条结构化通道。
    ///
    /// 覆写时应返回**确定性**结果（不依赖 LLM 输出），
    /// 否则无法写回归测试，也会让同一份输入产出不同结构。
    fn structured(&self, _ctx: &AgentContext, _messages: &[Message]) -> Option<Value> {
        None
    }
}

/// LLM 调用器：把重试、多轮工具调用与埋点收在一处
///
/// 各 agent 通过 `AgentContext::caller()` 取得，无需再逐项传递
/// base_url / api_key / model / 技能注册表，避免参数列表失控。
pub struct LlmCaller<'a> {
    client: &'a reqwest::Client,
    base_url: &'a str,
    api_key: &'a str,
    model: &'a str,
    /// 失败重试次数（0 = 不重试）
    max_retries: u32,
    /// 退避基数（毫秒）
    backoff_ms: u64,
    /// 工具调用最大轮数
    max_tool_rounds: usize,
    skills: Option<&'a SkillRegistry>,
    trace: Option<&'a TraceHandle>,
}

impl<'a> LlmCaller<'a> {
    /// 最简聊天补全（无工具）
    pub async fn chat(&self, system: &str, messages: &[Message]) -> Result<String> {
        let body = json!({
            "model": self.model,
            "messages": build_messages(system, messages),
            "temperature": 0.3,
            "stream": false,
        });
        let v = self.post(&body).await?;
        Ok(extract_content(&v))
    }

    /// 带工具的聊天补全：模型可在工具结果之上继续查证（`max_tool_rounds` 轮）
    ///
    /// 仅暴露该 capability 专属、或全局（无 owner）的技能；
    /// 若该 agent 没有可用工具，则自动退化为普通补全。
    pub async fn chat_with_tools(
        &self,
        system: &str,
        messages: &[Message],
        capability: Capability,
    ) -> Result<String> {
        let tools: Vec<Skill> = match self.skills {
            Some(reg) => reg
                .for_capability(capability)
                .into_iter()
                .cloned()
                .collect(),
            None => Vec::new(),
        };
        if tools.is_empty() {
            return self.chat(system, messages).await;
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

        let mut body_msgs = build_messages(system, messages);
        let rounds = self.max_tool_rounds.max(1);

        for round in 0..rounds {
            let body = json!({
                "model": self.model,
                "messages": body_msgs,
                "tools": tool_defs,
                "tool_choice": "auto",
                "temperature": 0.3,
                "stream": false,
            });
            let v = self.post(&body).await?;
            let msg = v
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("message"))
                .cloned()
                .unwrap_or(json!({}));

            let calls = msg
                .get("tool_calls")
                .and_then(|t| t.as_array())
                .filter(|a| !a.is_empty());
            // 模型不再要工具 → 本轮即为最终答案
            let Some(calls) = calls else {
                return Ok(extract_message_content(&msg));
            };

            body_msgs.push(msg.clone());
            for tc in calls {
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
                let id = tc
                    .get("id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_string();

                trace::record(self.trace, |m| m.record_tool(name));
                tracing::debug!(round = round + 1, tool = name, "模型请求调用工具");

                let result = match crate::skills::dispatch(&tools, name, &args).await {
                    Ok(v) => v,
                    Err(e) => {
                        let msg = e.to_string();
                        trace::record(self.trace, |m| m.record_error(msg.clone()));
                        json!({"error": msg})
                    }
                };
                body_msgs.push(json!({
                    "role": "tool",
                    "tool_call_id": id,
                    "content": result.to_string(),
                }));
            }

            // 已达轮数上限：不再给工具，改为一次纯汇总调用确保拿到最终文本
            if round + 1 >= rounds {
                tracing::debug!(rounds, "工具调用轮数已达上限，转为汇总调用");
                let body = json!({
                    "model": self.model,
                    "messages": body_msgs,
                    "temperature": 0.3,
                    "stream": false,
                });
                let v = self.post(&body).await?;
                return Ok(extract_content(&v));
            }
        }

        // 不可达：`rounds` 至少为 1，循环内必返回
        Ok(String::new())
    }

    /// 发一次 `chat/completions`：失败按 `is_retryable` 判定是否重试
    async fn post(&self, body: &Value) -> Result<Value> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut stat = LlmCallStat::default();
        let mut last: Option<anyhow::Error> = None;

        for attempt in 0..=self.max_retries {
            let started = Instant::now();
            let out = self.try_once(&url, body).await;
            stat.duration_ms += started.elapsed().as_millis();
            stat.attempts += 1;

            match out {
                Ok(v) => {
                    if let Some(u) = v.get("usage") {
                        stat.prompt_tokens = u.get("prompt_tokens").and_then(|x| x.as_u64());
                        stat.completion_tokens =
                            u.get("completion_tokens").and_then(|x| x.as_u64());
                        stat.total_tokens = u.get("total_tokens").and_then(|x| x.as_u64());
                    }
                    trace::record(self.trace, |m| m.record_llm(&stat));
                    return Ok(v);
                }
                Err(e) => {
                    let retryable = is_retryable(&e);
                    stat.error = Some(e.to_string());
                    last = Some(e);
                    if !retryable || attempt == self.max_retries {
                        break;
                    }
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_retries = self.max_retries,
                        error = %last.as_ref().unwrap(),
                        "LLM 调用失败，准备重试"
                    );
                    let wait = self.backoff_ms.saturating_mul(1u64 << attempt.min(4));
                    if wait > 0 {
                        tokio::time::sleep(Duration::from_millis(wait)).await;
                    }
                }
            }
        }

        trace::record(self.trace, |m| m.record_llm(&stat));
        Err(last.unwrap_or_else(|| anyhow::anyhow!("LLM 调用失败")))
    }

    /// 单次请求（不重试）
    async fn try_once(&self, url: &str, body: &Value) -> Result<Value> {
        let resp = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(body)
            .send()
            .await?
            .error_for_status()?;
        let v: Value = resp.json().await.context("解析 LLM 响应失败")?;
        Ok(v)
    }
}

/// 构造 messages 数组（可选 system + 对话历史）
fn build_messages(system: &str, messages: &[Message]) -> Vec<Value> {
    let mut body_msgs = Vec::new();
    if !system.is_empty() {
        body_msgs.push(json!({"role": "system", "content": system}));
    }
    for m in messages {
        body_msgs.push(json!({"role": m.role, "content": m.content}));
    }
    body_msgs
}

/// 从响应体取最终文本
fn extract_content(v: &Value) -> String {
    v.get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(extract_message_content_ref)
        .unwrap_or_default()
}

/// 从 assistant message 取文本（content 可能为 null）
fn extract_message_content(msg: &Value) -> String {
    extract_message_content_ref(msg).unwrap_or_default()
}

fn extract_message_content_ref(msg: &Value) -> Option<String> {
    msg.get("content")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
}

/// 判定失败是否值得重试
///
/// 重试：超时 / 连接失败 / 5xx / 429（多为瞬时抖动，重试大概率成功）。
/// 不重试：其余 4xx（模型不存在、参数非法等语义错误，重试只会浪费时间）。
fn is_retryable(e: &anyhow::Error) -> bool {
    let Some(re) = e.downcast_ref::<reqwest::Error>() else {
        return false;
    };
    if re.is_timeout() || re.is_connect() {
        return true;
    }
    matches!(
        re.status(),
        Some(s) if s.is_server_error() || s == reqwest::StatusCode::TOO_MANY_REQUESTS
    )
}
