//! MCP（Model Context Protocol）客户端：Streamable HTTP 传输
//!
//! 复刻 backend `app/mcp/`：以 JSON-RPC over HTTP 对接 MCP server。
//! 提供 `call_tool` 直接调用远端工具。

use anyhow::Result;
use serde_json::{json, Value};
use std::time::Duration;

/// 调用 MCP server 上的一个工具
///
/// `url` 为 MCP Streamable HTTP 端点（如 http://localhost:9000/mcp）。
/// 使用标准 JSON-RPC 2.0：`tools/call`。
pub async fn call_tool(
    client: &reqwest::Client,
    url: &str,
    tool: &str,
    args: &Value,
) -> Result<Value> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": tool,
            "arguments": args,
        }
    });

    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .timeout(Duration::from_secs(60))
        .json(&body)
        .send()
        .await?
        .error_for_status()?;

    let text = resp.text().await?;
    // 兼容 SSE 与纯 JSON 两种返回
    let text = if let Some(stripped) = text.strip_prefix("data:") {
        stripped.trim()
    } else {
        text.trim()
    };
    let v: Value = serde_json::from_str(text)
        .or_else(|_| serde_json::from_str::<Value>(&format!("{{\"result\":{text}}}")))
        .unwrap_or(json!({"result": null}));

    Ok(v
        .get("result")
        .cloned()
        .unwrap_or(json!(null)))
}

/// 列出 MCP server 提供的工具
pub async fn list_tools(client: &reqwest::Client, url: &str) -> Result<Value> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    });
    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .timeout(Duration::from_secs(30))
        .json(&body)
        .send()
        .await?
        .error_for_status()?;
    let text = resp.text().await?;
    let v: Value = serde_json::from_str(text.trim())
        .unwrap_or(json!({"result": []}));
    Ok(v.get("result").cloned().unwrap_or(json!([])))
}
