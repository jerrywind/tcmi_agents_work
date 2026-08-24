//! 云端中继服务器核心。
//!
//! 职责：
//! 1. `POST /api/register`  —— 家庭端带 `name` + `token` 注册，返回其应连接的 `ws_url`。
//! 2. `GET  /ws/:name`     —— 家庭端维持的持久控制连接（WebSocket，需 token）。
//! 3. `ANY  /t/:name/*rest`—— 外部调用方访问某个隧道，server 通过对应 WS 把请求转给家庭端，
//!                            再把家庭端回传的响应返回给调用方。
//! nginx 通常作为 TLS 终端与反代前置在本服务之前。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::{Body, Bytes},
    extract::{Path, Query, Request, State, WebSocketUpgrade},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures::{SinkExt, Stream, StreamExt};
use serde::Deserialize;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{error, info, warn};

use crate::protocol::{
    is_hop_by_hop_str, ClientToServer, RequestMsg, ResponseChunkMsg, ServerToClient,
};
use crate::skill::{JudgeError, SkillSet};
use crate::state::{PendingResponse, Registry, TunnelCommand};

/// 隧道名 → token 的鉴权表（来自配置文件）。
#[derive(Clone)]
pub struct TunnelAuth {
    tokens: Arc<HashMap<String, String>>,
}

impl TunnelAuth {
    pub fn from_list(list: &[(String, String)]) -> Self {
        Self {
            tokens: Arc::new(list.iter().cloned().collect()),
        }
    }
    pub fn check(&self, name: &str, token: &str) -> bool {
        match self.tokens.get(name) {
            Some(t) => t == token,
            None => false,
        }
    }
    pub fn contains(&self, name: &str) -> bool {
        self.tokens.contains_key(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // 提供 oneshot

    fn auth() -> TunnelAuth {
        TunnelAuth::from_list(&[("home".into(), "s3cr3t".into())])
    }

    fn test_state() -> AppState {
        AppState {
            registry: Registry::new(),
            auth: auth(),
            external_ws_base: "ws://127.0.0.1/rr".into(),
            skills: None,
        }
    }

    #[test]
    fn rejects_unknown_name() {
        assert!(!auth().check("other", "s3cr3t"));
    }

    #[test]
    fn rejects_wrong_token() {
        assert!(!auth().check("home", "wrong"));
    }

    #[test]
    fn accepts_valid_credentials() {
        assert!(auth().check("home", "s3cr3t"));
    }

    #[test]
    fn contains_reflects_configured_names() {
        let a = auth();
        assert!(a.contains("home"));
        assert!(!a.contains("nope"));
    }

    #[test]
    fn register_constructs_external_ws_url() {
        let state = AppState {
            registry: Registry::new(),
            auth: auth(),
            external_ws_base: "wss://rr.example.com/rr".into(),
            skills: None,
        };
        let ws_url = format!(
            "{}/ws/{}?token={}",
            state.external_ws_base, "home", "s3cr3t"
        );
        assert_eq!(ws_url, "wss://rr.example.com/rr/ws/home?token=s3cr3t");
    }

    #[tokio::test]
    async fn healthz_returns_ok() {
        let app = build_router(test_state());
        let resp = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn register_with_bad_token_returns_403() {
        let app = build_router(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/register")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"home","token":"nope"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn register_with_good_token_returns_200_and_ws_url() {
        let app = build_router(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/register")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"home","token":"s3cr3t"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value =
            serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let ws_url = v["ws_url"].as_str().expect("ws_url present");
        assert!(ws_url.starts_with("ws://127.0.0.1/rr/ws/home"));
        assert!(ws_url.contains("token=s3cr3t"));
    }

    #[tokio::test]
    async fn proxy_to_unknown_tunnel_returns_404() {
        // 隧道名未配置进鉴权表，proxy_handler 第一步 contains 校验即返回 404
        let app = build_router(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/t/unknown/foo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn proxy_to_known_but_disconnected_tunnel_returns_502() {
        // 隧道名在鉴权表中，但尚无 WS 连接，应返回 502 而非挂起
        let app = build_router(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/t/home/foo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn proxy_headers_exclude_hop_by_hop() {
        // proxy_handler 向下游构造 RequestMsg 时须过滤逐跳头；
        // 这里验证 headers 收集逻辑本身（通过已连接的隧道无法在纯单测里完成，
        // 故断言 is_hop_by_hop_str 过滤在 protocol/server 两侧一致）
        let headers: Vec<(String, String)> = vec![
            ("Content-Type".into(), "application/json".into()),
            ("Connection".into(), "keep-alive".into()),
            ("X-Trace".into(), "abc".into()),
        ];
        let filtered: Vec<(String, String)> = headers
            .iter()
            .filter(|(k, _)| !is_hop_by_hop_str(k))
            .cloned()
            .collect();
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|(k, _)| k != "Connection"));
    }

    // ---- 可选技能闸门（X-Skill 头）----
    use crate::skill::{ConstState, JudgeEngine, SkillRule, SkillSet};
    use std::sync::Arc;
    use std::time::Duration;

    fn gated_state(skill: SkillRule) -> AppState {
        let set = Arc::new(SkillSet::new(Arc::new(JudgeEngine::new(
            1_000_000,
            Arc::new(ConstState("idle".into())),
        ))));
        set.register(skill);
        AppState {
            registry: Registry::new(),
            auth: auth(),
            external_ws_base: "ws://127.0.0.1/rr".into(),
            skills: Some(set),
        }
    }

    // 注册一个「已知但无连接」的隧道，使闸门通过后落到 502 而非 404/403
    async fn with_disconnected_tunnel(state: AppState) -> AppState {
        let (tx, _rx) = mpsc::unbounded_channel();
        state.registry.register("home", tx).await;
        state
    }

    #[tokio::test]
    async fn proxy_gate_unknown_skill_returns_400() {
        let state = gated_state(SkillRule {
            name: "fire".into(),
            cooldown: Duration::from_secs(1),
            cost: 1,
            required_state: Some("idle".into()),
        });
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/t/home/foo")
                    .header("x-skill", "ghost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn proxy_gate_allows_then_blocks_on_cooldown() {
        let state = gated_state(SkillRule {
            name: "fire".into(),
            cooldown: Duration::from_secs(60),
            cost: 1,
            required_state: Some("idle".into()),
        });
        let state = with_disconnected_tunnel(state).await;
        let app = build_router(state);
        // 第一次：闸门通过 → 转发 → 隧道未连接 → 502
        let r1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/t/home/foo")
                    .header("x-skill", "fire")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r1.status(), StatusCode::BAD_GATEWAY);
        // 第二次（冷却内）：闸门拦截 → 429
        let r2 = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/t/home/foo")
                    .header("x-skill", "fire")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r2.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn proxy_skips_gate_when_no_x_skill_header() {
        // 即使配置了 SkillSet，未携带 X-Skill 头时闸门不生效，请求照常转发
        let state = gated_state(SkillRule {
            name: "fire".into(),
            cooldown: Duration::from_secs(60),
            cost: 1,
            required_state: Some("idle".into()),
        });
        let state = with_disconnected_tunnel(state).await;
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/t/home/foo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }
}

/// 服务器共享状态。
#[derive(Clone)]
pub struct AppState {
    pub registry: Registry,
    pub auth: TunnelAuth,
    /// 外部可达的 WS 基址，用于构造家庭端的 ws_url（如 wss://rr.example.com/rr）。
    pub external_ws_base: String,
    /// 可选技能闸门：配置了 `SkillSet` 后，带 `X-Skill` 头的请求会做冷却 / 资源 / 状态校验。
    /// `None` 表示不启用闸门（所有请求按原逻辑直接转发）。
    pub skills: Option<Arc<SkillSet>>,
}

#[derive(Deserialize)]
pub struct RegisterReq {
    pub name: String,
    pub token: String,
}

#[derive(Deserialize)]
pub struct WsQuery {
    pub token: String,
}

/// 零依赖 CORS 中间件：给所有响应补上跨域头，并对 OPTIONS 预检直接返回 204。
/// 便于 OpenAI 风格的浏览器/跨域客户端直接调用 `/t/<name>/...`。
fn cors_headers(headers: &mut HeaderMap) {
    let _ = headers.insert(
        HeaderName::from_static("access-control-allow-origin"),
        HeaderValue::from_static("*"),
    );
    let _ = headers.insert(
        HeaderName::from_static("access-control-allow-methods"),
        HeaderValue::from_static("GET, POST, PUT, DELETE, PATCH, OPTIONS"),
    );
    let _ = headers.insert(
        HeaderName::from_static("access-control-allow-headers"),
        HeaderValue::from_static("*"),
    );
}

async fn cors_middleware(req: Request, next: Next) -> Response {
    if req.method() == Method::OPTIONS {
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .header("access-control-allow-origin", "*")
            .header(
                "access-control-allow-methods",
                "GET, POST, PUT, DELETE, PATCH, OPTIONS",
            )
            .header("access-control-allow-headers", "*")
            .header("access-control-max-age", "86400")
            .body(Body::empty())
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }
    let mut resp = next.run(req).await;
    cors_headers(resp.headers_mut());
    resp
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/register", post(register_handler))
        .route("/ws/:name", get(ws_handler))
        .route(
            "/t/:name/*rest",
            get(proxy_handler)
                .post(proxy_handler)
                .put(proxy_handler)
                .delete(proxy_handler)
                .patch(proxy_handler),
        )
        // CORS：便于 OpenAI 风格的浏览器/跨域客户端直接调用 /t/<name>/...。
        // 用零依赖的手动中间件实现（含 OPTIONS 预检应答），避免引入额外依赖。
        .layer(middleware::from_fn(cors_middleware))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

pub async fn run_server(listen: String, state: AppState) -> anyhow::Result<()> {
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    info!("rrserver listening on {}", listen);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn register_handler(State(state): State<AppState>, Json(req): Json<RegisterReq>) -> impl IntoResponse {
    if !state.auth.check(&req.name, &req.token) {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "bad token"}))).into_response();
    }
    let ws_url = format!("{}/ws/{}?token={}", state.external_ws_base, req.name, req.token);
    (
        StatusCode::OK,
        Json(serde_json::json!({ "name": req.name, "ws_url": ws_url })),
    )
        .into_response()
}

async fn ws_handler(
    Path(name): Path<String>,
    Query(q): Query<WsQuery>,
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Response {
    if !state.auth.check(&name, &q.token) {
        return (StatusCode::FORBIDDEN, "bad token").into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(socket, name, state.registry))
}

async fn handle_socket(socket: axum::extract::ws::WebSocket, name: String, reg: Registry) {
    use axum::extract::ws::Message;
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<TunnelCommand>();
    reg.register(&name, tx.clone()).await;
    info!("tunnel '{}' connected", name);

    let reg_clone = reg.clone();
    // 发送任务：心跳 + 转发云端下发的请求。
    let mut send_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(25));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if sender.send(Message::Text(r#"{"type":"ping"}"#.into())).await.is_err() {
                        break;
                    }
                }
                cmd = rx.recv() => {
                    match cmd {
                        Some(TunnelCommand::Request(r)) => {
                            let msg = ServerToClient::Request(r);
                            match serde_json::to_string(&msg) {
                                Ok(s) => {
                                    if sender.send(Message::Text(s.into())).await.is_err() {
                                        break;
                                    }
                                }
                                Err(e) => warn!("serialize request failed: {}", e),
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    });

    // 接收任务：解析家庭端回传的响应并唤醒等待方。
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Text(t) => handle_client_msg(&reg_clone, &t).await,
                Message::Binary(b) => {
                    if let Ok(t) = String::from_utf8(b) {
                        handle_client_msg(&reg_clone, &t).await;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }
    // 条件清理：同名新连接可能已经替换本连接的注册项（register 的 insert 覆盖语义），
    // 此时旧连接退出时绝不能误删新隧道，否则重连成功的隧道会被踢下线。
    reg.unregister_if_same(&name, &tx).await;
    info!("tunnel '{}' disconnected", name);
}

async fn handle_client_msg(reg: &Registry, text: &str) {
    if let Ok(c2s) = serde_json::from_str::<ClientToServer>(text) {
        match c2s {
            ClientToServer::Response(r) => reg.resolve(r).await,
            ClientToServer::ResponseChunk(c) => reg.push_chunk(c).await,
            ClientToServer::Pong => {}
        }
    }
}

async fn proxy_handler(
    Path((name, rest)): Path<(String, String)>,
    uri: Uri,
    method: Method,
    headers: HeaderMap,
    State(state): State<AppState>,
    body: Bytes,
) -> Response {
    if !state.auth.contains(&name) {
        return (StatusCode::NOT_FOUND, "unknown tunnel").into_response();
    }

    // 可选技能闸门：带 `X-Skill` 头的请求先做冷却 / 资源 / 状态校验。
    // 闸门独立于隧道鉴权——仅当配置了 SkillSet 且请求显式携带该头时生效。
    // 校验通过（trigger 成功）才继续转发；失败则按原因映射不同状态码。
    if let Some(skills) = &state.skills {
        if let Some(skill_name) = headers.get("x-skill").and_then(|v| v.to_str().ok()) {
            match skills.trigger(skill_name) {
                Ok(_) => {}
                Err(JudgeError::CooldownActive { remaining }) => {
                    warn!(
                        "skill gate blocked `{}`: on cooldown ({}s remaining)",
                        skill_name,
                        remaining.as_secs_f64()
                    );
                    return (
                        StatusCode::TOO_MANY_REQUESTS,
                        format!(
                            "skill `{}` on cooldown ({}s remaining)",
                            skill_name,
                            remaining.as_secs_f64()
                        ),
                    )
                        .into_response();
                }
                Err(JudgeError::InsufficientResource { have, need }) => {
                    warn!(
                        "skill gate blocked `{}`: insufficient resource (have {}, need {})",
                        skill_name, have, need
                    );
                    return (
                        StatusCode::PAYMENT_REQUIRED,
                        format!(
                            "skill `{}` insufficient resource: have {}, need {}",
                            skill_name, have, need
                        ),
                    )
                        .into_response();
                }
                Err(JudgeError::StateMismatch { expected, actual }) => {
                    warn!(
                        "skill gate blocked `{}`: state mismatch (expected {}, actual {})",
                        skill_name, expected, actual
                    );
                    return (
                        StatusCode::CONFLICT,
                        format!(
                            "skill `{}` state mismatch: expected {}, actual {}",
                            skill_name, expected, actual
                        ),
                    )
                        .into_response();
                }
                Err(JudgeError::UnknownSkill(name)) => {
                    warn!("skill gate blocked: unknown skill `{}`", name);
                    return (StatusCode::BAD_REQUEST, format!("unknown skill: `{}`", name))
                        .into_response();
                }
                Err(JudgeError::Internal(m)) => {
                    error!("skill gate internal error: {}", m);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("skill gate internal error: {}", m),
                    )
                        .into_response();
                }
            }
        }
    }

    let query = uri.query().unwrap_or("");
    // axum 的 `*rest` 捕获不含前导 `/`，转发到本地服务时需补回
    let full_path = if query.is_empty() {
        format!("/{}", rest)
    } else {
        format!("/{}?{}", rest, query)
    };

    let hdr_list: Vec<(String, String)> = headers
        .iter()
        .filter(|(k, _)| !is_hop_by_hop_str(k.as_str()))
        .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.as_str().to_string(), s.to_string())))
        .collect();

    let req_id = uuid::Uuid::new_v4().to_string();
    let req = RequestMsg {
        req_id: req_id.clone(),
        method: method.to_string(),
        path: full_path,
        headers: hdr_list,
        body: body.to_vec(),
    };

    let mut rx = state.registry.new_pending(&req_id).await;
    let mut chunk_rx = state.registry.open_stream(&req_id).await;
    if !state.registry.send_request(&name, req).await {
        state.registry.cancel_pending(&req_id).await;
        return (StatusCode::BAD_GATEWAY, "tunnel not connected").into_response();
    }

    // 家庭端可回传完整 Response（非流式）或一串 ResponseChunk（流式，LLM 增量输出）。
    // 用 select! 抢占先到的那种；LLM 推理可能很慢且很长，故给足 600s 超时，
    // nginx 侧 proxy_read_timeout 需大于此值。
    enum Outcome {
        Full(Result<PendingResponse, ()>),
        Chunk(Option<ResponseChunkMsg>),
    }

    let outcome = timeout(Duration::from_secs(600), async {
        tokio::select! {
            full = &mut rx => Outcome::Full(full.map_err(|_| ())),
            first = chunk_rx.recv() => Outcome::Chunk(first),
        }
    })
    .await;

    match outcome {
        Err(_) => {
            // 超时：清理两侧登记，避免等待方/通道泄漏
            state.registry.cancel_pending(&req_id).await;
            (StatusCode::GATEWAY_TIMEOUT, "tunnel timeout").into_response()
        }
        Ok(Outcome::Full(Ok(pending))) => {
            state.registry.cancel_stream(&req_id).await;
            build_response(pending)
        }
        Ok(Outcome::Full(Err(_))) => {
            // 不应到达：oneshot 仅在流式路径胜出后才被取消
            state.registry.cancel_stream(&req_id).await;
            (StatusCode::BAD_GATEWAY, "tunnel closed").into_response()
        }
        Ok(Outcome::Chunk(None)) => {
            state.registry.cancel_pending(&req_id).await;
            (StatusCode::BAD_GATEWAY, "tunnel stream closed").into_response()
        }
        Ok(Outcome::Chunk(Some(first))) => {
            // 流式：首片提供 status/headers；后续片只含 chunk
            let status = if first.status == 0 { 200 } else { first.status };
            let mut builder = Response::builder().status(status);
            for (k, v) in &first.headers {
                // 逐跳头与 content-length 不透传：流式响应长度未知，由分块传输决定
                if is_hop_by_hop_str(k) || k.eq_ignore_ascii_case("content-length") {
                    continue;
                }
                if let (Ok(n), Ok(val)) = (
                    HeaderName::from_bytes(k.as_bytes()),
                    HeaderValue::from_str(v),
                ) {
                    builder = builder.header(n, val);
                }
            }
            let stream = ChunkStream {
                rx: chunk_rx,
                head: if first.chunk.is_empty() {
                    None
                } else {
                    Some(Bytes::from(first.chunk))
                },
                reg: state.registry.clone(),
                req_id: req_id.clone(),
            };
            builder
                .body(Body::from_stream(stream))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}

/// 由 `PendingResponse` 构造 HTTP 响应（过滤逐跳头）。
fn build_response(pending: PendingResponse) -> Response {
    let mut builder = Response::builder().status(pending.status);
    for (k, v) in pending.headers {
        // 响应侧同样过滤逐跳头，避免把 Connection/Upgrade 等透传给调用方
        if is_hop_by_hop_str(&k) {
            continue;
        }
        if let (Ok(n), Ok(val)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(&v),
        ) {
            builder = builder.header(n, val);
        }
    }
    builder
        .body(Body::from(pending.body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// 把家庭端回传的 `ResponseChunk` 通道转成 axum 的流式响应体。
/// `head` 为已取出的首片数据；后续片逐个吐出；末片（`done=true`）结束流。
struct ChunkStream {
    rx: mpsc::UnboundedReceiver<ResponseChunkMsg>,
    head: Option<Bytes>,
    reg: Registry,
    req_id: String,
}

impl Stream for ChunkStream {
    type Item = Result<Bytes, Box<dyn std::error::Error + Send + Sync>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(b) = this.head.take() {
                return Poll::Ready(Some(Ok(b)));
            }
            match this.rx.poll_recv(cx) {
                Poll::Ready(Some(c)) => {
                    if c.done {
                        if c.chunk.is_empty() {
                            // 流正常结束：回收可能遗留的 oneshot 与流式通道
                            let _ = this.reg.cancel_pending(&this.req_id);
                            let _ = this.reg.cancel_stream(&this.req_id);
                            return Poll::Ready(None);
                        }
                        // 末片携带数据：吐出数据并清理通道（cancel_stream 已在 push_chunk(done) 执行，
                        // 这里再次保险清理 oneshot 等待）
                        let _ = this.reg.cancel_pending(&this.req_id);
                        return Poll::Ready(Some(Ok(Bytes::from(c.chunk))));
                    } else if c.chunk.is_empty() {
                        continue; // 跳过空的非末片
                    } else {
                        return Poll::Ready(Some(Ok(Bytes::from(c.chunk))));
                    }
                }
                Poll::Ready(None) => {
                    // 隧道断开 / 流异常结束：回收可能遗留的等待与通道，避免泄漏
                    let _ = this.reg.cancel_pending(&this.req_id);
                    let _ = this.reg.cancel_stream(&this.req_id);
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
