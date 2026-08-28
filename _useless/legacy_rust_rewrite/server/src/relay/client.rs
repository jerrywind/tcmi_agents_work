//! 家庭端隧道客户端（与 rrserver/src/client.rs 一致）。
//!
//! 负责：连接云端 WS、发送 REGISTER、监听云端下发的 Request、转发到本地服务、
//! 并把本地响应（含 SSE 流式）逐块经 WS 回传。

use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
use tracing::{error, info, warn};

use super::protocol::{Chunk, Frame, Request, Response};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    /// 云端地址，如 wss://example.com/rr/ws/myname
    pub server: String,
    /// 隧道名（与云端凭证一致）
    pub name: String,
    /// 注册 token（与云端凭证一致）
    pub token: String,
    /// 本地被代理的服务地址，如 http://127.0.0.1:8080
    pub local: String,
    /// 重连间隔（秒）
    #[serde(default = "default_reconnect")]
    pub reconnect_secs: u64,
}

fn default_reconnect() -> u64 {
    5
}

/// 运行家庭端 client（阻塞，带重连）。
pub async fn run_client(cfg: ClientConfig) -> anyhow::Result<()> {
    let mut backoff = 1u64;
    loop {
        match connect_once(&cfg).await {
            Ok(_) => {
                backoff = 1;
                warn!("client 连接断开，准备重连");
            }
            Err(e) => {
                error!("client 连接失败: {e}");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(
            cfg.reconnect_secs.max(backoff),
        ))
        .await;
        backoff = (backoff * 2).min(60);
    }
}

async fn connect_once(cfg: &ClientConfig) -> anyhow::Result<()> {
    let url = cfg.server.clone();
    let mut req = url.into_client_request()?;
    let name = cfg.name.clone();
    let token = cfg.token.clone();
    req.headers_mut().insert(
        "sec-websocket-protocol",
        format!("register,{},{}", name, token).parse().unwrap(),
    );

    let (ws, _) = tokio_tungstenite::connect_async(req).await?;
    info!("client 已连接 {} (name={name})", cfg.server);
    let (mut wtx, mut wrx) = ws.split();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
    // 后台：把需要回传的帧发到 WS
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if wtx.send(m).await.is_err() {
                break;
            }
        }
    });

    let local = cfg.local.clone();
    let out_tx = Arc::new(Mutex::new(tx));

    while let Some(msg) = wrx.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                error!("ws recv error: {e}");
                break;
            }
        };
        match msg {
            Message::Text(t) => {
                let frame: Frame = match serde_json::from_str(&t) {
                    Ok(f) => f,
                    Err(e) => {
                        error!("bad frame: {e}");
                        continue;
                    }
                };
                if let Frame::Request(req) = frame {
                    let out_tx = out_tx.clone();
                    let local = local.clone();
                    tokio::spawn(async move {
                        if let Err(e) = forward_to_ws(req, &local, out_tx).await {
                            error!("forward error: {e}");
                        }
                    });
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    writer.abort();
    Ok(())
}

/// 把云端下发的 Request 转发到本地服务，并把响应逐块回传（SSE 流式友好）。
async fn forward_to_ws(
    req: Request,
    local: &str,
    out_tx: Arc<Mutex<tokio::sync::mpsc::UnboundedSender<Message>>>,
) -> anyhow::Result<()> {
    let target = rewrite_local(&req.url, local)?;
    let client = reqwest::Client::new();
    let mut builder = client
        .request(
            reqwest::Method::from_bytes(req.method.as_bytes())?,
            &target,
        )
        .body(req.body.clone());
    for (k, v) in &req.headers {
        if !k.eq_ignore_ascii_case("host") && !k.eq_ignore_ascii_case("content-length") {
            builder = builder.header(k, v);
        }
    }
    let resp = builder.send().await?;
    let status = resp.status().as_u16();
    let mut headers = std::collections::HashMap::new();
    for (k, v) in resp.headers().iter() {
        if let Ok(s) = v.to_str() {
            headers.insert(k.as_str().to_string(), s.to_string());
        }
    }
    let first = Response {
        rid: req.rid.clone(),
        status,
        headers,
    };
    {
        let tx = out_tx.lock().await;
        tx.send(Message::Text(serde_json::to_string(&Frame::Response(first))?))?;
    }
    let mut stream = resp.bytes_stream();
    loop {
        match stream.next().await {
            Some(Ok(chunk)) => {
                let c = Chunk {
                    rid: req.rid.clone(),
                    done: false,
                    bytes: chunk.to_vec(),
                };
                let tx = out_tx.lock().await;
                tx.send(Message::Text(serde_json::to_string(&Frame::Chunk(c))?))?;
            }
            Some(Err(e)) => {
                error!("body stream error: {e}");
                break;
            }
            None => break,
        }
    }
    let last = Chunk {
        rid: req.rid.clone(),
        done: true,
        bytes: vec![],
    };
    let tx = out_tx.lock().await;
    tx.send(Message::Text(serde_json::to_string(&Frame::Chunk(last))?))?;
    Ok(())
}

/// 把云端下发的完整 URL 改写为本地地址（替换 scheme://host:port）。
fn rewrite_local(url: &str, local: &str) -> anyhow::Result<String> {
    // url 形如 http://<server>:<port>/path；把 <server>:<port> 替换为 local 的 host:port
    let rest = match url.split_once("://") {
        Some((_, r)) => r,
        None => url,
    };
    let path = match rest.find('/') {
        Some(i) => &rest[i..],
        None => "/",
    };
    let local_clean = local.trim_end_matches('/');
    Ok(format!("{local_clean}{path}"))
}
