//! 隧道协议消息定义（与 rrserver/src/protocol.rs 一致）。
//!
//! 云端 server 与家庭端 client 通过 WebSocket 传递三类消息：
//! - `Request`  ：云端下发 HTTP 请求（方法与完整 URL，SSE 流式透传时请求体已在 body）
//! - `Chunk`    ：client 回传的响应/响应体分片（流式逐块）
//! - `Response` ：client 回传的状态行 + 响应头（首片）

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 云端 -> 家庭端：下发一个 HTTP 请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// 唯一标识一次请求（用于关联 Chunk 流）
    pub rid: String,
    /// 方法（GET/POST/...）
    pub method: String,
    /// 完整本地目标 URL（家庭端把 server:port 换成 127.0.0.1）
    pub url: String,
    /// 请求头
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// 已收集的请求体（流式请求体罕见，整段上传；SSE 关注响应侧流式）
    #[serde(default)]
    pub body: Vec<u8>,
}

/// 家庭端 -> 云端：回传响应首部（状态码 + 响应头）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub rid: String,
    pub status: u16,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

/// 家庭端 -> 云端：响应/响应体分片（流式逐块）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub rid: String,
    /// 是否最后一帧
    pub done: bool,
    #[serde(default)]
    pub bytes: Vec<u8>,
}

/// WebSocket 文本帧里承载的协议负载（JSON 包装 + 类型标签）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "d")]
pub enum Frame {
    Request(Request),
    Response(Response),
    Chunk(Chunk),
}

impl Frame {
    pub fn rid(&self) -> &str {
        match self {
            Frame::Request(r) => &r.rid,
            Frame::Response(r) => &r.rid,
            Frame::Chunk(r) => &r.rid,
        }
    }
}

/// 把 reqwest 的状态码转成 u16（便于序列化）。
pub fn status_to_u16(s: StatusCode) -> u16 {
    s.as_u16()
}

/// 把 u16 解析回 StatusCode。
pub fn u16_to_status(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}
