//! 云端中继 server（与 rrserver/src/server.rs 一致）。
//!
//! 提供：
//! - `POST /api/register`：家庭端获取 WS 注册地址（name+token 校验凭证）
//! - `GET /ws/:name`：家庭端控制 WebSocket 连接（注册隧道）
//! - `GET /t/:name/*`：外部经隧道反代到家庭端本地服务（**已支持 SSE 流式透传**）
//! - `GET /healthz`：探活（兼容 rrserver）

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    body::{Body, Bytes},
    extract::{
        Path,
        State,
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, Router},
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};
use tracing::{error, info};

use super::protocol::{Chunk, Frame, Request, Response as ProtoResponse, u16_to_status};
use super::skill::SkillSet;
use super::state::{Registry, send_request, Tunnel};

/// 云端中继共享状态。
#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<Registry>,
    pub auth: TunnelAuth,
    pub external_ws_base: String,
    pub skills: Option<Arc<SkillSet>>,
    /// 隧道转发协调器（首部/Chunk 的等待与缓冲）。
    pub coordinator: Arc<super::state::ForwardCoordinator>,
}

/// name -> (token) 凭证表（来自 config 的 [[tunnels]]）。
#[derive(Clone)]
pub struct TunnelAuth {
    tokens: HashMap<String, String>,
}

impl TunnelAuth {
    pub fn from_list(list: &[(String, String)]) -> Self {
        let mut tokens = HashMap::new();
        for (n, t) in list {
            tokens.insert(n.clone(), t.clone());
        }
        TunnelAuth { tokens }
    }
    pub fn check(&self, name: &str, token: &str) -> bool {
        match self.tokens.get(name) {
            Some(t) => t == token,
            None => false,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RegisterResp {
    pub ws_url: String,
    pub tunnel: String,
}

/// 家庭端调用：用 name+token 换取 WS 注册地址。
async fn api_register(
    State(st): State<AppState>,
    axum::extract::Json(body): axum::extract::Json<RegisterReq>,
) -> Response {
    if !st.auth.check(&body.name, &body.token) {
        return (StatusCode::UNAUTHORIZED, "invalid name/token").into_response();
    }
    let base = if st.external_ws_base.is_empty() {
        "wss://<your-domain>/rr".to_string()
    } else {
        st.external_ws_base.trim_end_matches('/').to_string()
    };
    let ws_url = format!("{base}/ws/{}", body.name);
    let resp = RegisterResp {
        ws_url,
        tunnel: body.name,
    };
    axum::Json(resp).into_response()
}

#[derive(Debug, Deserialize)]
pub struct RegisterReq {
    pub name: String,
    pub token: String,
}

/// 健康探活（兼容 rrserver）。
async fn healthz() -> &'static str {
    "ok"
}

/// 家庭端控制 WS：完成注册后进入消息循环。
async fn ws_handler(
    Path(name): Path<String>,
    State(st): State<AppState>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, name, st))
}

async fn handle_ws(socket: WebSocket, name: String, st: AppState) {
    // 简化阶段 A：不在 WS 升级阶段强制校验 token（注册由 /api/register 完成）。
    // 生产应加强：在 upgrade 前用 sec-websocket-protocol 校验。
    let (mut wtx, mut wrx) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<axum::extract::ws::Message>();
    if !st.registry.register(&name, tx) {
        error!("注册失败（重复或超上限）: {name}");
        let _ = wtx.send(WsMessage::Close(None)).await;
        return;
    }
    info!("隧道已注册: {name}（当前 {} 条）", st.registry.count());

    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if wtx.send(m).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = wrx.next().await {
        match msg {
            WsMessage::Text(t) => {
                let frame: Frame = match serde_json::from_str(&t) {
                    Ok(f) => f,
                    Err(e) => {
                        error!("bad frame: {e}");
                        continue;
                    }
                };
                // 云端只接收家庭端回传的 Response / Chunk；Request 由云端发出。
                match frame {
                    Frame::Response(_) | Frame::Chunk(_) => {
                        // 这些帧由转发逻辑持有 sender，这里转交给对应等待者。
                        st.coordinator.deliver(frame);
                    }
                    _ => {}
                }
            }
            WsMessage::Close(_) => break,
            _ => {}
        }
    }
    writer.abort();
    st.registry.unregister(&name);
    info!("隧道已注销: {name}");
}

/// 外部请求经隧道反代（流式）。
async fn tunnel_http(
    Path((name, rest)): Path<(String, String)>,
    State(st): State<AppState>,
    method: axum::http::Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let tunnel = match st.registry.get(&name) {
        Some(t) => t,
        None => return (StatusCode::NOT_FOUND, "tunnel not found").into_response(),
    };
    let rid = uuid::Uuid::new_v4().to_string();
    let target = format!("http://{name}/{rest}");
    let mut hmap = HashMap::new();
    for (k, v) in headers.iter() {
        if let Ok(s) = v.to_str() {
            hmap.insert(k.as_str().to_string(), s.to_string());
        }
    }
    let req = Request {
        rid: rid.clone(),
        method: method.to_string(),
        url: target,
        headers: hmap,
        body: body.to_vec(),
    };
    // 把请求交给隧道（异步等待回传的 Response/Chunk 流）
    let rx_resp = st.coordinator.register_waiter(&rid);
    if !send_request(&tunnel, req) {
        st.coordinator.unregister_waiter(&rid);
        return (StatusCode::BAD_GATEWAY, "send to tunnel failed").into_response();
    }

    // 等待首部
    let first = match tokio::time::timeout(std::time::Duration::from_secs(30), rx_resp).await {
        Ok(Ok(f)) => f,
        _ => return (StatusCode::GATEWAY_TIMEOUT, "tunnel timeout").into_response(),
    };
    let Frame::Response(resp) = first else {
        return (StatusCode::BAD_GATEWAY, "bad tunnel response").into_response();
    };
    let status = u16_to_status(resp.status);
    let mut builder = axum::response::Response::builder().status(status);
    for (k, v) in &resp.headers {
        builder = builder.header(k, v);
    }
    // 流式拼接后续 Chunk
    let rid2 = rid.clone();
    let coord = st.coordinator.clone();
    let stream = async_stream::stream! {
        while let Some(chunk) = coord.next_chunk(&rid2) {
            if chunk.done {
                break;
            }
            yield Ok::<_, std::io::Error>(Bytes::from(chunk.bytes));
        }
    };
    builder.body(Body::from_stream(stream)).unwrap()
}

/// 构造中继路由。
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/register", post(api_register))
        .route("/ws/:name", get(ws_handler))
        .route("/t/:name/*rest", get(tunnel_http))
        .with_state(state)
}
