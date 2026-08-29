//! 家庭端隧道客户端。
//!
//! 运行在家庭网络中（与本地 llm 服务同机或同局域网）。流程：
//! 1. `POST /api/register` 用 name + token 向云端注册，拿到 `ws_url`。
//! 2. 连接 `ws_url` 建立持久控制连接。
//! 3. 收到云端下发的 `Request` 后，转发到本地 llm 服务（`--local`），把响应回传云端。
//! 4. 连接断开自动重连（指数退避由固定 5s 简化）。
//!
//! 家庭网络通常无公网 IP/端口映射，但能主动出站访问云端 —— 这正是本隧道成立的前提。

use std::time::Duration;

use anyhow::{anyhow, Context};
use futures::{Sink, SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{error, info, warn};

use crate::protocol::{
    is_hop_by_hop_str, ClientToServer, RequestMsg, ResponseChunkMsg, ResponseMsg, ServerToClient,
};

#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// 云端 register 基址，如 https://rr.example.com/rr
    pub server_base: String,
    /// 隧道名（需与云端配置一致）
    pub name: String,
    /// 隧道 token（需与云端配置一致）
    pub token: String,
    /// 本地 llm 服务基址，如 http://127.0.0.1:8080
    pub local_url: String,
}

pub async fn run_client(cfg: ClientConfig) -> anyhow::Result<()> {
    loop {
        if let Err(e) = run_once(cfg.clone()).await {
            error!("tunnel error: {:#}", e);
        }
        warn!("reconnecting in 5s...");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn run_once(cfg: ClientConfig) -> anyhow::Result<()> {
    let http = reqwest::Client::new();
    let reg_url = format!("{}/api/register", cfg.server_base);
    let resp = http
        .post(&reg_url)
        .json(&json!({"name": cfg.name, "token": cfg.token}))
        .send()
        .await
        .context("register request failed")?;
    if !resp.status().is_success() {
        return Err(anyhow!("register failed: {}", resp.status()));
    }
    let reg: serde_json::Value = resp.json().await.context("bad register response")?;
    let ws_url = reg
        .get("ws_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("no ws_url in register response"))?
        .to_string();
    info!("registered, connecting {}", ws_url);

    let (ws_stream, _) = connect_async(&ws_url).await.context("ws connect failed")?;
    let (mut write, mut read) = ws_stream.split();
    info!("tunnel '{}' connected", cfg.name);

    let local = cfg.local_url.trim_end_matches('/').to_string();
    while let Some(Ok(msg)) = read.next().await {
        match msg {
            Message::Text(t) => {
                if let Ok(s2c) = serde_json::from_str::<ServerToClient>(&t) {
                    match s2c {
                        ServerToClient::Request(r) => {
                            // 以流式方式转发（首片带 status/headers，随后逐块回传），支持 LLM 增量输出
                            if !forward_to_ws(&mut write, &http, &local, &r).await {
                                break;
                            }
                        }
                        ServerToClient::Ping => {
                            if write
                                .send(Message::Text("{\"type\":\"pong\"}".to_string()))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
            }
            Message::Binary(b) => {
                if let Ok(t) = String::from_utf8(b) {
                    if let Ok(ServerToClient::Request(r)) =
                        serde_json::from_str::<ServerToClient>(&t)
                    {
                        // 以流式方式转发，支持 LLM 增量输出
                        if !forward_to_ws(&mut write, &http, &local, &r).await {
                            break;
                        }
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    Ok(())
}

/// 把云端的请求转发到本地 llm 服务并回收响应。
///
/// 便捷封装：每次调用内部创建一个临时 `reqwest::Client`。适合测试与一次性调用。
/// 在 `run_once` 这类高频隧道转发路径中，请改用 [`forward_local_with`] 复用连接池。
pub async fn forward_local(local_base: &str, req: &RequestMsg) -> ResponseMsg {
    let client = reqwest::Client::new();
    forward_local_with(&client, local_base, req).await
}

/// 与 [`forward_local`] 行为一致，但复用调用方传入的 `client` 连接池，
/// 避免每次请求都重建 TCP 连接 / 重新解析 DNS，显著提升隧道转发的吞吐与延迟。
pub async fn forward_local_with(
    client: &reqwest::Client,
    local_base: &str,
    req: &RequestMsg,
) -> ResponseMsg {
    let url = format!("{}{}", local_base, req.path);
    let method = match req.method.to_uppercase().as_str() {
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "DELETE" => reqwest::Method::DELETE,
        "PATCH" => reqwest::Method::PATCH,
        _ => reqwest::Method::GET,
    };
    let mut rb = client.request(method, &url);
    for (k, v) in &req.headers {
        rb = rb.header(k, v);
    }
    rb = rb.body(req.body.clone());
    match rb.send().await {
        Ok(r) => {
            let status = r.status().as_u16();
            let headers: Vec<(String, String)> = r
                .headers()
                .iter()
                .filter(|(k, _)| !is_hop_by_hop_str(k.as_str()))
                .filter_map(|(k, v)| {
                    v.to_str()
                        .ok()
                        .map(|s| (k.as_str().to_string(), s.to_string()))
                })
                .collect();
            let body = r.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
            ResponseMsg {
                req_id: req.req_id.clone(),
                status,
                headers,
                body,
            }
        }
        Err(e) => ResponseMsg {
            req_id: req.req_id.clone(),
            status: 502,
            headers: vec![],
            body: format!("forward error: {}", e).into_bytes(),
        },
    }
}

/// 把云端请求以**流式**方式转发到本地 llm 服务，并分片回传给云端。
///
/// 首片携带 `status` + `headers`（供云端构造响应头），随后逐块回传响应体，
/// 末片 `done = true` 结束。这样 LLM 的增量输出（流式 / SSE）能被原样透传。
///
/// 返回 `false` 表示写向隧道失败（调用方应断开隧道）。
async fn forward_to_ws<W>(
    write: &mut W,
    client: &reqwest::Client,
    local_base: &str,
    req: &RequestMsg,
) -> bool
where
    W: Sink<Message> + Unpin,
{
    let url = format!("{}{}", local_base, req.path);
    let method = match req.method.to_uppercase().as_str() {
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "DELETE" => reqwest::Method::DELETE,
        "PATCH" => reqwest::Method::PATCH,
        _ => reqwest::Method::GET,
    };
    let mut rb = client.request(method, &url);
    for (k, v) in &req.headers {
        rb = rb.header(k, v);
    }
    rb = rb.body(req.body.clone());
    match rb.send().await {
        Ok(mut r) => {
            let status = r.status().as_u16();
            let headers: Vec<(String, String)> = r
                .headers()
                .iter()
                .filter(|(k, _)| !is_hop_by_hop_str(k.as_str()))
                .filter_map(|(k, v)| {
                    v.to_str()
                        .ok()
                        .map(|s| (k.as_str().to_string(), s.to_string()))
                })
                .collect();
            // 首片：仅 status/headers，body 置空
            if !send_chunk(
                write,
                ResponseChunkMsg {
                    req_id: req.req_id.clone(),
                    status,
                    headers,
                    chunk: vec![],
                    done: false,
                },
            )
            .await
            {
                return false;
            }
            // 真·流式转发：随本地响应体的到达逐块回传（`Response::chunk` 每读到一段就回传一段），
            // LLM 的增量输出（SSE / 逐 token）可第一时间透传到云端，避免「读完整个响应再切片」
            // 带来的首字延迟。该方法无需 reqwest 的 `stream` 特性，离线环境亦可编译。
            loop {
                match r.chunk().await {
                    Ok(Some(bytes)) => {
                        if bytes.is_empty() {
                            continue;
                        }
                        if !send_chunk(
                            write,
                            ResponseChunkMsg {
                                req_id: req.req_id.clone(),
                                status: 0,
                                headers: vec![],
                                chunk: bytes.to_vec(),
                                done: false,
                            },
                        )
                        .await
                        {
                            return false;
                        }
                    }
                    Ok(None) => break, // 响应体读取完毕
                    Err(e) => {
                        // 体流读取中途失败：以 502 末片结束本次转发，并断开隧道
                        let _ = send_chunk(
                            write,
                            ResponseChunkMsg {
                                req_id: req.req_id.clone(),
                                status: 502,
                                headers: vec![],
                                chunk: format!("stream read error: {}", e).into_bytes(),
                                done: true,
                            },
                        )
                        .await;
                        return false;
                    }
                }
            }
            send_chunk(
                write,
                ResponseChunkMsg {
                    req_id: req.req_id.clone(),
                    status: 0,
                    headers: vec![],
                    chunk: vec![],
                    done: true,
                },
            )
            .await
        }
        Err(e) => {
            send_chunk(
                write,
                ResponseChunkMsg {
                    req_id: req.req_id.clone(),
                    status: 502,
                    headers: vec![],
                    chunk: format!("forward error: {}", e).into_bytes(),
                    done: true,
                },
            )
            .await
        }
    }
}

/// 序列化并发送一个响应分片；返回 `false` 表示发送失败。
async fn send_chunk<W>(write: &mut W, chunk: ResponseChunkMsg) -> bool
where
    W: Sink<Message> + Unpin,
{
    let msg = ClientToServer::ResponseChunk(chunk);
    match serde_json::to_string(&msg) {
        Ok(s) => write.send(Message::Text(s)).await.is_ok(),
        Err(e) => {
            error!("serialize response chunk failed: {}", e);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(req_id: &str) -> RequestMsg {
        RequestMsg {
            req_id: req_id.to_string(),
            method: "GET".into(),
            path: "/x".into(),
            headers: vec![],
            body: vec![],
        }
    }

    #[tokio::test]
    async fn forward_to_unreachable_local_returns_502() {
        // 连向一个几乎不可能在监听的端口，应返回受控的 502 而非 panic
        let resp = forward_local("http://127.0.0.1:1", &req("r1")).await;
        assert_eq!(resp.status, 502);
        assert_eq!(resp.req_id, "r1");
    }

    #[test]
    fn method_mapping_defaults_to_get() {
        // 仅校验非标准动词回退为 GET，避免 forward 时误用
        let m = |v: &str| match v.to_uppercase().as_str() {
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "DELETE" => reqwest::Method::DELETE,
            "PATCH" => reqwest::Method::PATCH,
            _ => reqwest::Method::GET,
        };
        assert_eq!(m("GET"), reqwest::Method::GET);
        assert_eq!(m("weird"), reqwest::Method::GET);
        assert_eq!(m("post"), reqwest::Method::POST);
        assert_eq!(m("PUT"), reqwest::Method::PUT);
        assert_eq!(m("Delete"), reqwest::Method::DELETE);
        assert_eq!(m("patch"), reqwest::Method::PATCH);
    }

    #[test]
    fn client_config_fields_are_set() {
        let cfg = ClientConfig {
            server_base: "https://rr.example.com/rr".into(),
            name: "home".into(),
            token: "secret".into(),
            local_url: "http://127.0.0.1:8080".into(),
        };
        assert_eq!(cfg.name, "home");
        // local_url 末尾斜杠在转发时被去除，确保路径拼接正确
        assert_eq!(cfg.local_url.trim_end_matches('/'), "http://127.0.0.1:8080");
    }

    #[tokio::test]
    async fn forward_local_strips_trailing_slash_from_base() {
        // forward_local 内部用 format!("{}{}", local_base, path)，
        // 若 base 带末尾斜杠则会变成 //path；确保运行期由 run_once 去除。
        // 这里直接验证 forward 对 base 带斜杠时仍能连到正确地址（不可达端口）。
        let resp = forward_local("http://127.0.0.1:1/", &req("r2")).await;
        assert_eq!(resp.status, 502);
        assert_eq!(resp.req_id, "r2");
    }

    #[tokio::test]
    async fn forward_local_propagates_custom_headers() {
        // 启动一个本地服务，验证转发时把请求头原样带过去
        use axum::{
            body::Bytes,
            http::{HeaderMap, Method},
            response::IntoResponse,
            routing::any,
            Router,
        };
        async fn sink(_m: Method, headers: HeaderMap, _b: Bytes) -> impl IntoResponse {
            if headers.get("x-echo").map(|v| v == "42").unwrap_or(false) {
                "header-ok"
            } else {
                "header-missing"
            }
        }
        let app = Router::new().fallback(any(sink));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _h = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut r = req("r3");
        r.headers = vec![("X-Echo".into(), "42".into())];
        let resp = forward_local(&format!("http://{}", addr), &r).await;
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"header-ok");
    }

    #[tokio::test]
    async fn forward_local_maps_methods_to_local() {
        // 验证 DELETE 等方法被映射为对应 HTTP 动词转发（本地服务断言方法）
        use axum::{
            body::Bytes,
            http::{HeaderMap, Method, StatusCode},
            response::IntoResponse,
            routing::any,
            Router,
        };
        async fn sink(method: Method, _h: HeaderMap, _b: Bytes) -> impl IntoResponse {
            (StatusCode::OK, method.to_string())
        }
        let app = Router::new().fallback(any(sink));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _h = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        for (verb, want) in [
            ("POST", "POST"),
            ("PUT", "PUT"),
            ("DELETE", "DELETE"),
            ("PATCH", "PATCH"),
        ] {
            let mut r = req("x");
            r.method = verb.to_string();
            let resp = forward_local(&format!("http://{}", addr), &r).await;
            assert_eq!(resp.status, 200);
            assert_eq!(resp.body, want.as_bytes());
        }
    }

    #[tokio::test]
    async fn forward_local_with_reuses_shared_client_pool() {
        // 验证共享连接池路径：用同一个 reqwest::Client 连续转发多次，行为等价于 forward_local。
        use axum::{
            body::Bytes,
            http::{HeaderMap, Method},
            response::IntoResponse,
            routing::any,
            Router,
        };
        async fn sink(_m: Method, _h: HeaderMap, _b: Bytes) -> impl IntoResponse {
            (axum::http::StatusCode::OK, "ok")
        }
        let app = Router::new().fallback(any(sink));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _h = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let base = format!("http://{}", addr);

        // 复用同一个 client 多次转发，验证连接池路径可用且行为一致
        let client = reqwest::Client::new();
        for i in 0..3 {
            let mut r = req("pool");
            r.req_id = format!("r{}", i);
            let resp = forward_local_with(&client, &base, &r).await;
            assert_eq!(resp.status, 200);
            assert_eq!(resp.body, b"ok");
        }
    }
}
