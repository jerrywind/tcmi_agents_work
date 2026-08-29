//! MCP Server 端（T4.5）：把 harness 的 7 个能力暴露给外部 MCP 客户端
//!
//! 与 [`crate::mcp`](super) 的 client 同仓反向：client 是「把外部工具接进来」，
//! server 是「把本系统能力交出去」，供 Claude Desktop / Cursor 等 MCP 客户端调用。
//!
//! 传输与 client 一致：**Streamable HTTP**（JSON-RPC 2.0 over `POST /mcp`），
//! 不引入第三方 SDK，也不改编排器——`tools/call` 直接翻译成一次
//! [`crate::orchestrator::run_single`]，复用现有的资源、技能与埋点链路。
//!
//! 无会话状态：每次调用自带完整 `messages`，调用方自行维护多轮。

use crate::model::{Capability, Message};
use crate::orchestrator;
use crate::AppState;
use serde_json::{json, Value};

/// 实现的 MCP 协议版本
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// JSON-RPC 错误码：方法不存在
pub const METHOD_NOT_FOUND: i32 = -32601;
/// JSON-RPC 错误码：参数无效
pub const INVALID_PARAMS: i32 = -32602;

/// 通用入口工具名：一次调用指定 capability
pub const RUN_AGENT_TOOL: &str = "run_agent";
/// 能力清单工具名（不需要 LLM，纯查表）
pub const LIST_CAPABILITIES_TOOL: &str = "list_agent_capabilities";

/// 某个 capability 对应的 MCP 工具名：`agent_<slug>`
pub fn agent_tool_name(cap: Capability) -> String {
    format!("agent_{}", cap.slug())
}

/// 对外暴露的工具清单（`tools/list` 的返回）
pub fn tool_definitions() -> Vec<Value> {
    let mut tools: Vec<Value> = Capability::ALL
        .iter()
        .map(|cap| {
            json!({
                "name": agent_tool_name(*cap),
                "description": format!("中医「{}」Sub-Agent：{}", cap.zh(), capability_desc(*cap)),
                "inputSchema": agent_input_schema(false),
            })
        })
        .collect();
    tools.push(json!({
        "name": RUN_AGENT_TOOL,
        "description": "通用入口：按 capability 名调用任意一个 Sub-Agent",
        "inputSchema": agent_input_schema(true),
    }));
    tools.push(json!({
        "name": LIST_CAPABILITIES_TOOL,
        "description": "列出全部可用的 Sub-Agent 能力（slug 与中文名），无需 LLM",
        "inputSchema": json!({"type": "object", "properties": {}}),
    }));
    tools
}

fn capability_desc(cap: Capability) -> &'static str {
    match cap {
        Capability::Inspection => "抽取神色形态与舌象特征",
        Capability::Listening => "从声音与气味判断虚实寒热",
        Capability::Inquiry => "围绕寒热汗出头身二便等系统追问",
        Capability::Palpation => "解读脉象与体检数据",
        Capability::Differentiation => "综合四诊辨证，产出主证/兼证与置信度",
        Capability::Safety => "识别急危重症信号与用药禁忌",
        Capability::Treatment => "给出方剂、外治与调护建议",
    }
}

/// `agent_*` 工具的入参 schema；`with_capability` 用于通用入口（需额外指定 capability）
fn agent_input_schema(with_capability: bool) -> Value {
    let mut properties = serde_json::Map::new();
    if with_capability {
        properties.insert(
            "capability".to_string(),
            json!({
                "type": "string",
                "description": "能力 slug，如 differentiation；也接受中文名（如 辨证）",
                "enum": Capability::ALL.iter().map(|c| c.slug()).collect::<Vec<_>>(),
            }),
        );
    }
    properties.insert(
        "messages".to_string(),
        json!({
            "type": "array",
            "description": "完整对话历史（harness 无会话状态，每次需自带上下文）",
            "items": {
                "type": "object",
                "properties": {
                    "role": {"type": "string", "enum": ["user", "assistant", "system"]},
                    "content": {"type": "string"},
                },
                "required": ["role", "content"],
            },
        }),
    );
    properties.insert(
        "payload".to_string(),
        json!({"type": "object", "description": "可选附加数据（体质档案等），透传给 Sub-Agent"}),
    );

    let required = if with_capability {
        vec!["capability".to_string(), "messages".to_string()]
    } else {
        vec!["messages".to_string()]
    };
    json!({"type": "object", "properties": properties, "required": required})
}

/// 处理一条 JSON-RPC 请求
///
/// 返回 `None` 表示这是**通知**（notification，无 `id`）：按 MCP 规范不回响应，
/// 由路由层转成 204 No Content。
pub async fn handle(state: &AppState, req: &Value) -> Option<Value> {
    // 通知（如 notifications/initialized）没有 id，协议要求不回包；
    // `?` 把「无 id」直接转成 `handle` 的 None（函数返回 Option）。
    let id = req.get("id").cloned()?;
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");

    let outcome = match method {
        "initialize" => Ok(initialize_result()),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_definitions()})),
        "tools/call" => call_tool(state, req.get("params")).await,
        other => Err(error(METHOD_NOT_FOUND, format!("未知方法: {other}"))),
    };

    Some(match outcome {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
    })
}

/// `initialize` 的返回：声明协议版本与能力
fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {
            "name": "tcm-harness",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

/// JSON-RPC 错误对象
fn error(code: i32, message: String) -> Value {
    json!({"code": code, "message": message})
}

/// 执行 `tools/call`
///
/// 区分两类失败：
/// - **协议层**（工具名不存在、参数非法）→ JSON-RPC `error`；
/// - **执行层**（Sub-Agent 跑挂了，多半是 LLM 不可达）→ 正常 result + `isError: true`，
///   让模型看得到失败原因并自行决定下一步，而不是拿到一条干瘪的错误码。
async fn call_tool(state: &AppState, params: Option<&Value>) -> Result<Value, Value> {
    let params = params.unwrap_or(&Value::Null);
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    if name == LIST_CAPABILITIES_TOOL {
        return Ok(text_result(&capabilities_text(state), None));
    }
    if name == RUN_AGENT_TOOL {
        let raw = args
            .get("capability")
            .and_then(|c| c.as_str())
            .unwrap_or("");
        let cap = Capability::from_name(raw).ok_or_else(|| {
            error(
                INVALID_PARAMS,
                format!("未知 capability: {raw}（用 {LIST_CAPABILITIES_TOOL} 查看可用值）"),
            )
        })?;
        return run_agent(state, cap, &args).await;
    }
    if let Some(slug) = name.strip_prefix("agent_") {
        let cap = Capability::from_slug(slug)
            .ok_or_else(|| error(INVALID_PARAMS, format!("未知工具: {name}")))?;
        return run_agent(state, cap, &args).await;
    }

    Err(error(INVALID_PARAMS, format!("未知工具: {name}")))
}

/// 跑一个 Sub-Agent，把结果包装成 MCP 的 `tools/call` 返回
async fn run_agent(state: &AppState, cap: Capability, args: &Value) -> Result<Value, Value> {
    let messages = parse_messages(args.get("messages"));
    if messages.is_empty() {
        return Err(error(
            INVALID_PARAMS,
            "messages 不能为空：harness 无会话状态，需自带完整对话历史".to_string(),
        ));
    }
    let payload = args.get("payload").cloned().unwrap_or_else(|| json!({}));

    let res = state.resources.read().await;
    match orchestrator::run_single(
        &state.registry,
        &state.config,
        &res,
        &state.llm,
        &state.skills,
        cap,
        &messages,
        &payload,
    )
    .await
    {
        Ok((_, text, _, structured)) => Ok(text_result(&text, structured)),
        Err(e) => Ok(json!({
            "content": [{"type": "text", "text": format!("调用失败：{e}")}],
            "isError": true,
        })),
    }
}

/// MCP 文本结果；有结构化输出时附带 `structuredContent`（供客户端直接取字段）
fn text_result(text: &str, structured: Option<Value>) -> Value {
    let mut result = json!({
        "content": [{"type": "text", "text": text}],
        "isError": false,
    });
    if let Some(v) = structured {
        result["structuredContent"] = v;
    }
    result
}

fn parse_messages(v: Option<&Value>) -> Vec<Message> {
    let Some(arr) = v.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|m| {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content = m.get("content").and_then(|c| c.as_str())?;
            Some(Message {
                role: role.to_string(),
                content: content.to_string(),
            })
        })
        .collect()
}

fn capabilities_text(state: &AppState) -> String {
    let caps = state.registry.capabilities();
    let lines: Vec<String> = caps
        .iter()
        .map(|c| format!("- {}（{}）", c.slug(), c.zh()))
        .collect();
    format!("可用能力 {} 个：\n{}", caps.len(), lines.join("\n"))
}
