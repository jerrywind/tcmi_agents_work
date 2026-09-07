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
use crate::skills::{Skill, SkillRegistry, RAG_SKILL};
use crate::stream::DeltaSink;
use crate::trace::{self, LlmCallStat, TraceHandle};
use futures::StreamExt;
use std::collections::BTreeMap;
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
    /// 典籍检索是否不可用（T7.9，由编排器依 `payload.rag_available` 注入）
    ///
    /// 为 true 时 `tcm-rag` 不下发给模型——见 [`tools_for_llm`]。
    /// 默认 false：单步调用（`/agents`）不带该字段时按可用处理，行为不变。
    pub rag_down: bool,
    /// token 增量出口（流式时由编排器注入；非流式调用为 None）
    ///
    /// 放在上下文里而不是作为参数逐层传递：`SubAgent::run` 的签名是固定的，
    /// 而 LLM 调用深埋在各 agent 内部，唯一的注入点就是上下文。
    pub delta: Option<DeltaSink>,
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
            rag_down: false,
            delta: None,
        }
    }

    /// 挂上 token 增量出口（流式步骤用）
    pub fn with_delta(mut self, delta: Option<DeltaSink>) -> Self {
        self.delta = delta;
        self
    }

    /// 标记典籍检索不可用（撤掉 `tcm-rag`，见 [`tools_for_llm`]）
    pub fn with_rag_down(mut self, down: bool) -> Self {
        self.rag_down = down;
        self
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
            llm_stream: self.config.llm_stream,
            skills: Some(&self.skills),
            trace: Some(&self.trace),
            rag_down: self.rag_down,
            delta: self.delta.clone(),
        }
    }
}

/// 下发给模型的工具清单（唯一入口）。
///
/// **RAG 不可用时撤掉 `tcm-rag`**：此时该工具的返回值恒为一条错误提示，
/// 而模型每调一次就要**多走一整轮** LLM（又一次 prefill + 一轮生成）。
/// 实测开方步 `llm_calls=2`、`llm_duration_ms=29373`，其中一次调用就耗在
/// 这个必然失败的工具上。撤掉它不仅省时间，还少一次「模型以为自己查过典籍」的误导。
///
/// 纯函数、不碰网络，故可直接单测（见 `tests/behavior.rs`）。
pub fn tools_for_llm(tools: Vec<Skill>, rag_down: bool) -> Vec<Skill> {
    if !rag_down {
        return tools;
    }
    tools.into_iter().filter(|s| s.name != RAG_SKILL).collect()
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
    /// 是否对上游开启 token 级流式（受 `config.llm_stream` 控制）
    llm_stream: bool,
    skills: Option<&'a SkillRegistry>,
    trace: Option<&'a TraceHandle>,
    /// 典籍检索是否不可用：为 true 时不下发 `tcm-rag`（见 [`tools_for_llm`]）
    rag_down: bool,
    /// token 增量出口（None 表示不推增量）
    delta: Option<DeltaSink>,
}

impl<'a> LlmCaller<'a> {
    /// 是否走 token 级流式：配置开启 **且** 有增量出口
    ///
    /// 只开配置没有出口时（如 `/chat`、MCP、llm_eval）仍然整包返回：
    /// 那些调用方没有 SSE 通道，流式只会白白多一次解析开销。
    ///
    /// ## 关于「流式会不会弄丢工具调用」
    ///
    /// 曾有一次实测看到开流式后 `Listening` / `Palpation` 的 `tool_calls` 为空、
    /// `llm_calls` 从 2 降到 1，一度判定「流式让模型不再请求工具」并据此
    /// 只在无工具轮次开流式。后来加大样本复核：**流式下工具调用是正常的**
    /// （同一次运行里 `Listening=tcm-auscultation`、`Palpation=tcm-palpation`，
    /// `llm_calls=2`）——前一次只是模型这次没选工具，**采样巧合，不是因果**。
    ///
    /// 教训：单次观察就下因果结论，会把一个可用的能力误杀。
    /// 真正需要防的不是流式本身，而是「上游不支持流式」这种环境差异，
    /// 那由 [`Self::try_once`] 按 Content-Type 兜底。
    fn stream_enabled(&self) -> bool {
        self.llm_stream && self.delta.is_some()
    }

    /// 给请求体打上流式标记
    fn with_stream(mut body: Value, on: bool) -> Value {
        if on {
            body["stream"] = json!(true);
            // 流式下 `usage` 默认不下发，不带上这个字段埋点的 token 会全变 null，
            // 直接破坏 T3.1 的埋点与 llm_eval 的成本统计。
            body["stream_options"] = json!({ "include_usage": true });
        } else {
            body["stream"] = json!(false);
        }
        body
    }

    /// 最简聊天补全（无工具）
    pub async fn chat(&self, system: &str, messages: &[Message]) -> Result<String> {
        let body = Self::with_stream(
            json!({
                "model": self.model,
                "messages": build_messages(system, messages),
                "temperature": 0.3,
            }),
            self.stream_enabled(),
        );
        let v = self.post(&body).await?;
        Ok(extract_content(&v))
    }

    /// 最简聊天补全（无工具，支持视觉图片）
    pub async fn chat_imgs(
        &self,
        system: &str,
        messages: &[Message],
        images: &[Value],
    ) -> Result<String> {
        let body = Self::with_stream(
            json!({
                "model": self.model,
                "messages": build_messages_with(system, messages, images),
                "temperature": 0.3,
            }),
            self.stream_enabled(),
        );
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
        self.chat_with_tools_imgs(system, messages, capability, &[])
            .await
    }

    /// 带工具的聊天补全（支持视觉图片）。
    ///
    /// `images` 是 OpenAI 视觉 content-part（`{"type":"image_url","image_url":{"url":...}}`），
    /// 会挂到消息历史里的**第一条 user 消息**上——舌象 / 手相等望诊依据应随主诉
    /// 一起进模型。空数组时等价于 [`LlmCaller::chat_with_tools`]，行为不变。
    ///
    /// 仅望诊 agent 在收到前端上传的图片时调用；其余 agent 走无图版本，互不影响。
    pub async fn chat_with_tools_imgs(
        &self,
        system: &str,
        messages: &[Message],
        capability: Capability,
        images: &[Value],
    ) -> Result<String> {
        let listed: Vec<Skill> = match self.skills {
            Some(reg) => reg
                .for_capability(capability)
                .into_iter()
                .cloned()
                .collect(),
            None => Vec::new(),
        };
        // 撤掉「必然失败」的工具（RAG 不可用时），省掉一整轮 LLM 调用
        let tools = tools_for_llm(listed, self.rag_down);
        if tools.is_empty() {
            return self.chat_imgs(system, messages, images).await;
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

        let mut body_msgs = build_messages_with(system, messages, images);
        let rounds = self.max_tool_rounds.max(1);

        for round in 0..rounds {
            let body = Self::with_stream(
                json!({
                    "model": self.model,
                    "messages": body_msgs,
                    "tools": tool_defs,
                    "tool_choice": "auto",
                    "temperature": 0.3,
                }),
                self.stream_enabled(),
            );
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

                // 带上调用方 capability：「按知识域检索」这类技能要据此
                // 决定检索范围（开方查方书、切诊查脉学）
                let result =
                    match crate::skills::dispatch(&tools, name, &args, Some(capability)).await {
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
                let body = Self::with_stream(
                    json!({
                        "model": self.model,
                        "messages": body_msgs,
                        "temperature": 0.3,
                    }),
                    self.stream_enabled(),
                );
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
                    // 流式的重试必须让前端看见：见 `DeltaSink::emit_retry`——
                    // 既要打破「6 分钟只有心跳」的静默，也要让它丢弃已推的 delta
                    // （重试会从头再推一遍，累加就重复了）。
                    if let Some(d) = &self.delta {
                        d.emit_retry(
                            attempt + 1,
                            self.max_retries,
                            &last.as_ref().unwrap().to_string(),
                        );
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
    ///
    /// 流式开启时在这里就把 SSE 消费掉，并**合成**一个与非流式同构的响应体
    /// （`choices[0].message` + `usage`）。这样上层的重试、埋点、工具分派
    /// 全部不必区分流式与否——少一条分支就少一处漏改。
    async fn try_once(&self, url: &str, body: &Value) -> Result<Value> {
        let streaming = body
            .get("stream")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let resp = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(body)
            .send()
            .await?
            .error_for_status()?;
        // **按响应的 Content-Type 决定怎么解析，而不是按请求的 `stream` 标志**。
        //
        // 有些上游（或中间层网关）会**忽略** `stream: true`，直接回一个完整 JSON。
        // 若此时仍按 SSE 解析，`find_frame` 永远找不到分隔符，结果是 content 为空、
        // usage 为空——步骤「成功」返回一段空正文。不报错、不失败，
        // 只是结论里凭空少了一整段，是最难查的一类静默失效。
        // 故以实际响应类型为准：确是 `text/event-stream` 才走流式解析。
        let is_sse = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_ascii_lowercase().contains("text/event-stream"))
            .unwrap_or(false);
        if is_sse {
            return self.consume_stream(resp).await;
        }
        if streaming {
            tracing::warn!("上游未返回 SSE（可能不支持 stream），已退回整包 JSON 解析");
        }
        let v: Value = resp.json().await.context("解析 LLM 响应失败")?;
        Ok(v)
    }

    /// 消费上游 SSE，逐片推给前端，并在结束时合成完整响应体
    ///
    /// **工具调用的分片重组**是这里唯一有风险的地方：流式的 `tool_calls`
    /// 是被切成很多片的（`arguments` 逐字符到达），必须按 `index` 累积后
    /// 再拼回完整的调用。拼错不会报错，只会让 RAG 检索静默不执行——
    /// 结论照常产出，只是少了典籍依据。故该能力由 `llm_stream` 开关控制，
    /// 且真机验证时要核对 `trace.tool_calls` 与关闭时一致。
    async fn consume_stream(&self, resp: reqwest::Response) -> Result<Value> {
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        let mut content = String::new();
        let mut usage: Option<Value> = None;
        let mut calls: BTreeMap<u64, ToolCallAcc> = BTreeMap::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("读取 LLM 流式响应失败")?;
            buf.extend_from_slice(&chunk);
            // 帧以空行分隔；最后不足一帧的残片留在 buf 里等下一块
            while let Some(pos) = find_frame(&buf) {
                let frame: Vec<u8> = buf.drain(..pos + 2).collect();
                let text = String::from_utf8_lossy(&frame);
                for line in text.lines() {
                    let Some(data) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let data = data.trim();
                    if data.is_empty() || data == "[DONE]" {
                        continue;
                    }
                    let Ok(v) = serde_json::from_str::<Value>(data) else {
                        continue;
                    };
                    if let Some(u) = v.get("usage") {
                        if u.is_object() {
                            usage = Some(u.clone());
                        }
                    }
                    let Some(delta) = v
                        .get("choices")
                        .and_then(|c| c.get(0))
                        .and_then(|c| c.get("delta"))
                    else {
                        continue;
                    };
                    // 正文分片：一边累积一边推给前端
                    if let Some(s) = delta.get("content").and_then(|c| c.as_str()) {
                        if !s.is_empty() {
                            content.push_str(s);
                            if let Some(d) = &self.delta {
                                d.push(s);
                            }
                        }
                    }
                    if let Some(arr) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                        for c in arr {
                            accumulate_call(&mut calls, c);
                        }
                    }
                }
            }
        }

        let tool_calls: Vec<Value> = calls
            .values()
            .filter(|c| !c.name.is_empty())
            .map(|c| {
                json!({
                    "id": c.id,
                    "type": "function",
                    "function": { "name": c.name, "arguments": c.args },
                })
            })
            .collect();

        Ok(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": content,
                    "tool_calls": tool_calls,
                }
            }],
            "usage": usage,
        }))
    }
}

/// 流式下的一个工具调用（分片累积中）
#[derive(Default)]
struct ToolCallAcc {
    id: String,
    name: String,
    args: String,
}

/// 把一片 `delta.tool_calls[i]` 累积进对应下标的调用
fn accumulate_call(calls: &mut BTreeMap<u64, ToolCallAcc>, c: &Value) {
    let idx = c.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
    let e = calls.entry(idx).or_default();
    if let Some(s) = c.get("id").and_then(|v| v.as_str()) {
        e.id.push_str(s);
    }
    if let Some(f) = c.get("function") {
        if let Some(s) = f.get("name").and_then(|v| v.as_str()) {
            e.name.push_str(s);
        }
        // `arguments` 是逐字符到达的，必须**追加**而不是覆盖
        if let Some(s) = f.get("arguments").and_then(|v| v.as_str()) {
            e.args.push_str(s);
        }
    }
}

/// 在字节缓冲里找 SSE 帧分隔符 `\n\n`，返回其下标
fn find_frame(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}

/// 构造 messages 数组（可选 system + 对话历史）
fn build_messages(system: &str, messages: &[Message]) -> Vec<Value> {
    build_messages_with(system, messages, &[])
}

/// 同 [`build_messages`]，但把 `images`（OpenAI 视觉 content-part）挂到第一条
/// user 消息上。无图时等价于 [`build_messages`]。
fn build_messages_with(system: &str, messages: &[Message], images: &[Value]) -> Vec<Value> {
    let mut body_msgs = Vec::new();
    if !system.is_empty() {
        body_msgs.push(json!({"role": "system", "content": system}));
    }
    let mut attached = false;
    for m in messages {
        if !attached && m.role == "user" && !images.is_empty() {
            let mut content = vec![json!({"type": "text", "text": m.content})];
            for img in images {
                content.push(img.clone());
            }
            body_msgs.push(json!({ "role": "user", "content": content }));
            attached = true;
        } else {
            body_msgs.push(json!({ "role": m.role, "content": m.content }));
        }
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
