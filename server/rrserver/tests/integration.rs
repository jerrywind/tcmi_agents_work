//! 端到端集成测试：启动真实 rrserver 与本地 mock llm 服务，验证反向隧道转发全链路。
//!
//! 覆盖：注册鉴权、未知/未连接隧道、二进制 body 无损、响应逐跳头过滤、本地不可达回退。

use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, Method, StatusCode},
    response::Response,
    routing::any,
    Router,
};
use futures::{SinkExt, StreamExt};
use rrserver::client::{forward_local, run_client};
use rrserver::llmsrv::{Backend, BackendConfig, Deployer, DeploymentConfig, RrClientConfig};
use rrserver::protocol::{ClientToServer, HeartbeatAck, ServerToClient};
use rrserver::registry::ServiceRegistry;
use rrserver::server::{build_router, AppState, HealthConfig, TunnelAuth};
use rrserver::skill::{ConstState, JudgeEngine, SkillRule, SkillSet};
use rrserver::state::Registry;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

/// 启动云端 rrserver，监听 127.0.0.1:0，返回 `ip:port` 与任务句柄。
/// 默认配置两条隧道：`home`/secret，便于多隧道隔离测试复用。
async fn start_server() -> (String, JoinHandle<()>) {
    start_server_with_health(HealthConfig::default()).await
}

/// 同上，但可自定义心跳 / 探活 / 转发超时（测试里用毫秒级阈值驱动）。
async fn start_server_with_health(health: HealthConfig) -> (String, JoinHandle<()>) {
    let (addr, _state, handle) = start_server_with_state(health).await;
    (addr, handle)
}

/// 再进一步：把 `AppState` 也交回给测试，便于直接操作注册中心（如模拟「隧道在但注册没了」）。
async fn start_server_with_state(
    health: HealthConfig,
) -> (String, rrserver::server::AppState, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let auth = TunnelAuth::from_list(&[
        ("home".into(), "secret".into()),
        ("other".into(), "othertok".into()),
    ]);
    let state = AppState {
        registry: Registry::new(),
        services: ServiceRegistry::new(),
        auth,
        // 接入端可达基址仅用于构造 ws_url；测试中我们直连 addr，不经过 nginx 剥离 /rr 前缀
        external_ws_base: format!("ws://{}", addr),
        skills: None,
        health,
        http: reqwest::Client::new(),
    };
    // 生产由 run_server 拉起回收任务；测试直连 build_router，这里显式拉起
    let _reaper = rrserver::server::spawn_reaper(state.clone());
    let app = build_router(state.clone());
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr.to_string(), state, handle)
}

/// 启动带技能闸门的云端 rrserver（便于「闸门 × 隧道」端到端测试复用）。
async fn start_skilled_server(skill: SkillRule) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let auth = TunnelAuth::from_list(&[
        ("home".into(), "secret".into()),
        ("other".into(), "othertok".into()),
    ]);
    let set = Arc::new(SkillSet::new(Arc::new(JudgeEngine::new(
        1_000_000,
        Arc::new(ConstState("idle".into())),
    ))));
    set.register(skill);
    let state = AppState {
        registry: Registry::new(),
        services: ServiceRegistry::new(),
        auth,
        external_ws_base: format!("ws://{}", addr),
        skills: Some(set),
        health: HealthConfig::default(),
        http: reqwest::Client::new(),
    };
    let app = build_router(state);
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr.to_string(), handle)
}

/// 启动一个本地 mock llm 服务：回显请求 body，固定 status 201，
/// 并故意带上逐跳头 `Connection` 以验证隧道转发侧会过滤。
async fn start_local_mock() -> (String, JoinHandle<()>) {
    async fn mock_echo(_method: Method, headers: HeaderMap, body: Bytes) -> Response {
        let mut builder = Response::builder().status(201);
        builder = builder.header("X-Mock", "yes");
        builder = builder.header("Connection", "keep-alive"); // 应被 proxy_handler 过滤
        if let Some(v) = headers.get("content-type") {
            builder = builder.header("Content-Type", v);
        }
        builder.body(Body::from(body)).unwrap()
    }
    let app = Router::new().fallback(any(mock_echo));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{}", addr), handle)
}

/// 家庭端隧道客户端模拟器：连上 WS，把云端下发的请求转发到本地 llm，回传响应。
/// 同时响应云端下发的 Ping（回 Pong）与 Heartbeat 探活（回 alive ack）。
///
/// 转发在独立任务中进行：这样「慢请求」不会挡住探活回应
/// ——真实 client 也是同样的并发结构。
async fn run_home(ws_url: String, local: String) {
    run_home_with_heartbeat(ws_url, local, true).await
}

async fn run_home_with_heartbeat(ws_url: String, local: String, alive: bool) {
    let (ws_stream, _resp) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let (mut w, mut r) = ws_stream.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ClientToServer>();
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let text = serde_json::to_string(&msg).unwrap();
            if w.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
    });
    while let Some(Ok(msg)) = r.next().await {
        match msg {
            Message::Text(t) => {
                match serde_json::from_str::<ServerToClient>(&t) {
                    Ok(ServerToClient::Request(req)) => {
                        let out = out_tx.clone();
                        let local = local.clone();
                        tokio::spawn(async move {
                            let resp = forward_local(&local, &req).await;
                            let _ = out.send(ClientToServer::Response(resp));
                        });
                    }
                    Ok(ServerToClient::Ping) => {
                        // 云端心跳，家庭端需回 Pong
                        if out_tx.send(ClientToServer::Pong).is_err() {
                            break;
                        }
                    }
                    Ok(ServerToClient::Heartbeat(p)) => {
                        // 云端探活：回 ack（alive 可由调用方指定，用于模拟失联）
                        if out_tx
                            .send(ClientToServer::Heartbeat(HeartbeatAck {
                                probe_id: p.probe_id,
                                alive,
                            }))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => {}
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    writer.abort();
}

async fn register(addr: &str, name: &str, token: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{}/api/register", addr))
        .json(&json!({"name": name, "token": token}))
        .send()
        .await
        .unwrap()
}

/// 轮询直到家庭端隧道真正就绪（connected 且能转发到本地），避免固定 sleep 的竞态。
/// 就绪判定：本地正常返回 200/201，或本地不可达返回带 "forward error" 的 502。
async fn wait_for_tunnel_connected(addr: &str) {
    let client = reqwest::Client::new();
    let base = format!("http://{}", addr);
    for _ in 0..100 {
        if let Ok(resp) = client.get(format!("{}/t/home/x", base)).send().await {
            let status = resp.status();
            if status == 200 || status == 201 {
                return;
            }
            if status == 502 {
                let body = resp.text().await.unwrap_or_default();
                if body.contains("forward error") {
                    return; // 隧道已连，仅本地不可达
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("tunnel did not become ready in time");
}

#[tokio::test]
async fn register_with_bad_token_is_forbidden() {
    let (addr, sh) = start_server().await;
    let resp = register(&addr, "home", "wrong").await;
    assert_eq!(resp.status(), 403);
    sh.abort();
}

#[tokio::test]
async fn register_with_good_token_returns_ws_url() {
    let (addr, sh) = start_server().await;
    let resp = register(&addr, "home", "secret").await;
    assert_eq!(resp.status(), 200);
    let v = resp.json::<serde_json::Value>().await.unwrap();
    let ws_url = v["ws_url"].as_str().expect("ws_url present");
    assert!(ws_url.contains("/ws/home"));
    assert!(ws_url.contains("token=secret"));
    sh.abort();
}

#[tokio::test]
async fn unknown_tunnel_returns_404() {
    let (addr, sh) = start_server().await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/t/none/x", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    sh.abort();
}

#[tokio::test]
async fn tunnel_registered_but_not_connected_returns_502() {
    let (addr, sh) = start_server().await;
    // 已配置 token 但尚无 WS 连接
    let resp = reqwest::Client::new()
        .get(format!("http://{}/t/home/x", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 502);
    sh.abort();
}

#[tokio::test]
async fn end_to_end_forwards_to_local_llm_and_filters_hop_by_hop() {
    let (addr, sh) = start_server().await;
    let (local, lh) = start_local_mock().await;

    let reg = register(&addr, "home", "secret").await;
    let ws_url = reg.json::<serde_json::Value>().await.unwrap()["ws_url"]
        .as_str()
        .unwrap()
        .to_string();
    let home = tokio::spawn(run_home(ws_url, local));
    // 等待隧道注册到 registry
    wait_for_tunnel_connected(&addr).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/t/home/v1/chat", addr))
        .header("Content-Type", "application/json")
        .body("hello")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 201);
    assert_eq!(resp.headers().get("x-mock").unwrap(), "yes");
    // 逐跳头不应透传给外部调用方
    assert!(resp.headers().get("connection").is_none());
    assert_eq!(resp.text().await.unwrap(), "hello");

    home.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn binary_body_preserved_end_to_end() {
    let (addr, sh) = start_server().await;
    let (local, lh) = start_local_mock().await;

    let reg = register(&addr, "home", "secret").await;
    let ws_url = reg.json::<serde_json::Value>().await.unwrap()["ws_url"]
        .as_str()
        .unwrap()
        .to_string();
    let home = tokio::spawn(run_home(ws_url, local));
    wait_for_tunnel_connected(&addr).await;

    // 含非 UTF-8 字节的负载，验证 base64 通道无损
    let binary = vec![0u8, 159, 146, 150, 255, 1, 2, 3];
    let resp = reqwest::Client::new()
        .post(format!("http://{}/t/home/v1/bin", addr))
        .body(binary.clone())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 201);
    assert_eq!(resp.bytes().await.unwrap().to_vec(), binary);

    home.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn local_unreachable_yields_502_through_tunnel() {
    let (addr, sh) = start_server().await;

    let reg = register(&addr, "home", "secret").await;
    let ws_url = reg.json::<serde_json::Value>().await.unwrap()["ws_url"]
        .as_str()
        .unwrap()
        .to_string();
    // 指向一个不可达的本地服务
    let home = tokio::spawn(run_home(ws_url, "http://127.0.0.1:1".to_string()));
    wait_for_tunnel_connected(&addr).await;

    let resp = reqwest::Client::new()
        .get(format!("http://{}/t/home/x", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 502);

    home.abort();
    sh.abort();
}

/// 启动一个会回显「请求路径 + query」的本地 mock，便于验证隧道转发的完整 URL。
async fn start_local_path_echo() -> (String, JoinHandle<()>) {
    async fn echo_path(method: Method, uri: axum::http::Uri, body: Bytes) -> Response {
        let payload = format!("{}|{}|{}", method, uri.path(), uri.query().unwrap_or(""));
        let mut out = payload.into_bytes();
        out.extend_from_slice(&body);
        Response::builder()
            .status(200)
            .header("Content-Type", "text/plain")
            .body(Body::from(out))
            .unwrap()
    }
    let app = Router::new().fallback(any(echo_path));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{}", addr), handle)
}

#[tokio::test]
async fn healthz_endpoint_returns_ok() {
    let (addr, sh) = start_server().await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/healthz", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "ok");
    sh.abort();
}

#[tokio::test]
async fn ws_handshake_with_wrong_token_is_rejected() {
    let (addr, sh) = start_server().await;
    // 用错误的 token 直接尝试 WS 升级，应得到 403
    let url = format!("ws://{}/ws/home?token=wrong", addr);
    let result = tokio_tungstenite::connect_async(&url).await;
    assert!(result.is_err(), "expected handshake rejection");
    sh.abort();
}

#[tokio::test]
async fn query_string_is_preserved_to_local() {
    let (addr, sh) = start_server().await;
    let (local, lh) = start_local_path_echo().await;

    let reg = register(&addr, "home", "secret").await;
    let ws_url = reg.json::<serde_json::Value>().await.unwrap()["ws_url"]
        .as_str()
        .unwrap()
        .to_string();
    let home = tokio::spawn(run_home(ws_url, local));
    wait_for_tunnel_connected(&addr).await;

    let resp = reqwest::Client::new()
        .get(format!(
            "http://{}/t/home/v1/chat?model=gpt&stream=true",
            addr
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    // 路径与 query 都应原样透传到本地服务
    assert!(body.starts_with("GET|/v1/chat|model=gpt&stream=true"));

    home.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn tunnels_are_isolated_by_name() {
    // home 与 other 是两条独立隧道；请求 /t/other 不应落到 home 的本地服务
    let (addr, sh) = start_server().await;
    let (home_local, hlh) = start_local_path_echo().await;
    let (other_local, olh) = start_local_path_echo().await;

    let home_reg = register(&addr, "home", "secret").await;
    let home_ws = home_reg.json::<serde_json::Value>().await.unwrap()["ws_url"]
        .as_str()
        .unwrap()
        .to_string();
    let other_reg = register(&addr, "other", "othertok").await;
    let other_ws = other_reg.json::<serde_json::Value>().await.unwrap()["ws_url"]
        .as_str()
        .unwrap()
        .to_string();

    let home = tokio::spawn(run_home(home_ws, home_local.clone()));
    let other = tokio::spawn(run_home(other_ws, other_local.clone()));
    wait_for_tunnel_connected(&addr).await;

    // 打到 other 隧道
    let resp = reqwest::Client::new()
        .get(format!("http://{}/t/other/whoami", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.starts_with("GET|/whoami|"),
        "should hit other tunnel's local, got: {body}"
    );

    home.abort();
    other.abort();
    sh.abort();
    hlh.abort();
    olh.abort();
}

#[tokio::test]
async fn pong_is_sent_in_response_to_ping() {
    // 模拟「云端」WS 服务端：家庭端连上后立即下发 {"type":"ping"}，
    // 期望家庭端回 {"type":"Pong"}。本测试验证 run_home（家庭端逻辑）对心跳的正确响应。
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let srv = tokio::spawn(async move {
        use tokio_tungstenite::{accept_async, tungstenite::Message as WsMsg};
        let (stream, _) = listener.accept().await.unwrap();
        let ws = accept_async(stream).await.unwrap();
        let (mut w, mut r) = ws.split();
        // 下发 ping
        w.send(WsMsg::Text(r#"{"type":"ping"}"#.into()))
            .await
            .unwrap();
        // 等待 Pong
        while let Some(Ok(msg)) = r.next().await {
            if let WsMsg::Text(t) = msg {
                if let Ok(c2s) = serde_json::from_str::<ClientToServer>(&t) {
                    if matches!(c2s, ClientToServer::Pong) {
                        return true;
                    }
                }
            }
        }
        false
    });

    let ws_url = format!("ws://{}/ws/home?token=secret", addr);
    let home = tokio::spawn(run_home(ws_url, "http://127.0.0.1:1".to_string()));

    let got_pong = tokio::time::timeout(Duration::from_secs(3), srv)
        .await
        .expect("timed out waiting for pong")
        .unwrap();
    assert!(got_pong, "home client should reply Pong to cloud Ping");

    home.abort();
}

#[tokio::test]
async fn request_method_is_forwarded_to_local() {
    let (addr, sh) = start_server().await;
    let (local, lh) = start_local_path_echo().await;

    let reg = register(&addr, "home", "secret").await;
    let ws_url = reg.json::<serde_json::Value>().await.unwrap()["ws_url"]
        .as_str()
        .unwrap()
        .to_string();
    let home = tokio::spawn(run_home(ws_url, local));
    wait_for_tunnel_connected(&addr).await;

    // DELETE 请求经隧道转发，本地应收到 DELETE 方法
    let resp = reqwest::Client::new()
        .delete(format!("http://{}/t/home/v1/resource/1", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.starts_with("DELETE|/v1/resource/1|"));

    home.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn skill_gate_passes_first_then_blocks_tunnel_on_cooldown() {
    // 闸门 × 隧道：带 X-Skill 的首请求放行并穿透隧道到达本地（201），
    // 冷却内的第二请求被闸门拦截（429），根本不会进隧道。
    let rule = SkillRule {
        name: "fire".into(),
        cooldown: Duration::from_secs(60),
        cost: 1,
        required_state: Some("idle".into()),
    };
    let (addr, sh) = start_skilled_server(rule).await;
    let (local, lh) = start_local_mock().await;

    let reg = register(&addr, "home", "secret").await;
    let ws_url = reg.json::<serde_json::Value>().await.unwrap()["ws_url"]
        .as_str()
        .unwrap()
        .to_string();
    let home = tokio::spawn(run_home(ws_url, local));
    wait_for_tunnel_connected(&addr).await;

    let client = reqwest::Client::new();
    let first = client
        .post(format!("http://{}/t/home/v1/chat", addr))
        .header("Content-Type", "application/json")
        .header("X-Skill", "fire")
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 201); // 闸门放行 + 隧道穿透到本地

    let second = client
        .post(format!("http://{}/t/home/v1/chat", addr))
        .header("Content-Type", "application/json")
        .header("X-Skill", "fire")
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 429); // 冷却内被闸门拦截

    home.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn llm_server_pipeline_deploys_static_and_tunnels_to_local() {
    // llm_server 全链路拉通：Deployer(static) 接入本地 mock + 注册到 rrserver
    // + 隧道穿透，外部请求经 /t/home 直达本地 mock（201）。
    let (addr, sh) = start_server().await;
    let (local, lh) = start_local_mock().await;

    let cfg = DeploymentConfig {
        backend: BackendConfig {
            backend: Backend::Static { url: local.clone() },
            health_timeout_secs: 5,
        },
        rrclient: RrClientConfig {
            server_base: format!("http://{}", addr),
            name: "home".into(),
            token: "secret".into(),
        },
        models: vec![],
        info_port: None,
    };
    let deployer = Deployer::new(cfg);
    let deployed = deployer.deploy().await.unwrap();
    assert_eq!(deployed.local_url, local);

    let client_cfg = deployer.build_client_config(&deployed.local_url);
    let home = tokio::spawn(run_client(client_cfg));
    wait_for_tunnel_connected(&addr).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{}/t/home/v1/chat", addr))
        .header("Content-Type", "application/json")
        .body("hi")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    assert_eq!(resp.text().await.unwrap(), "hi");

    home.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn streaming_response_reassembled_across_chunks() {
    // 真实家庭端 run_client 以流式分片回传；本地返回 >16KB 的 body，
    // 触发多分片（body.chunks(16KiB)），server 侧 ChunkStream 应正确重组为完整响应。
    let (addr, sh) = start_server().await;
    let (local, lh) = start_local_mock().await;

    let cfg = DeploymentConfig {
        backend: BackendConfig {
            backend: Backend::Static { url: local.clone() },
            health_timeout_secs: 5,
        },
        rrclient: RrClientConfig {
            server_base: format!("http://{}", addr),
            name: "home".into(),
            token: "secret".into(),
        },
        models: vec![],
        info_port: None,
    };
    let deployer = Deployer::new(cfg);
    let deployed = deployer.deploy().await.unwrap();
    let client_cfg = deployer.build_client_config(&deployed.local_url);
    let home = tokio::spawn(run_client(client_cfg));
    wait_for_tunnel_connected(&addr).await;

    let big = "x".repeat(50_000);
    let resp = reqwest::Client::new()
        .post(format!("http://{}/t/home/v1/chat", addr))
        .header("Content-Type", "application/json")
        .body(big.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    assert_eq!(resp.text().await.unwrap(), big);

    home.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn cors_preflight_options_returns_204() {
    let (addr, sh) = start_server().await;
    let resp = reqwest::Client::new()
        .request(
            reqwest::Method::OPTIONS,
            format!("http://{}/t/home/v1/chat", addr),
        )
        .header("Origin", "https://example.com")
        .header("Access-Control-Request-Method", "POST")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        resp.headers().get("access-control-allow-origin").unwrap(),
        "*"
    );
    sh.abort();
}

#[tokio::test]
async fn cors_actual_request_carries_allow_origin() {
    let (addr, sh) = start_server().await;
    // 隧道未连接返回 502，但 CORS 头仍应附加（便于浏览器识别错误信息）
    let resp = reqwest::Client::new()
        .get(format!("http://{}/t/home/x", addr))
        .header("Origin", "https://example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 502);
    assert_eq!(
        resp.headers().get("access-control-allow-origin").unwrap(),
        "*"
    );
    sh.abort();
}

/// 启动一个固定返回指定状态码、并回显 body 的本地 mock（用于验证非 2xx 透传）。
async fn start_local_mock_status(status: u16) -> (String, JoinHandle<()>) {
    let app = Router::new().fallback(any(move |body: Bytes| async move {
        Response::builder()
            .status(status)
            .header("X-Mock", "yes")
            .body(Body::from(body))
            .unwrap()
    }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{}", addr), handle)
}

/// 启动带「多个」技能闸门的云端 rrserver（便于验证冷却按技能名隔离）。
async fn start_skilled_server_multi(rules: Vec<SkillRule>) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let auth = TunnelAuth::from_list(&[
        ("home".into(), "secret".into()),
        ("other".into(), "othertok".into()),
    ]);
    let set = Arc::new(SkillSet::new(Arc::new(JudgeEngine::new(
        1_000_000,
        Arc::new(ConstState("idle".into())),
    ))));
    for r in rules {
        set.register(r);
    }
    let state = AppState {
        registry: Registry::new(),
        services: ServiceRegistry::new(),
        auth,
        external_ws_base: format!("ws://{}", addr),
        skills: Some(set),
        health: HealthConfig::default(),
        http: reqwest::Client::new(),
    };
    let app = build_router(state);
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr.to_string(), handle)
}

#[tokio::test]
async fn concurrent_requests_stay_isolated_on_same_tunnel() {
    // 同一条隧道并发处理多个请求，验证各自响应正确路由、body 互不串扰
    // （保护之前修复的 oneshot/stream `select!` 竞态）。
    let (addr, sh) = start_server().await;
    let (local, lh) = start_local_path_echo().await;

    let reg = register(&addr, "home", "secret").await;
    let ws_url = reg.json::<serde_json::Value>().await.unwrap()["ws_url"]
        .as_str()
        .unwrap()
        .to_string();
    let home = tokio::spawn(run_home(ws_url, local));
    wait_for_tunnel_connected(&addr).await;

    let mut handles = vec![];
    for i in 0..5 {
        let url = format!("http://{}/t/home/v1/chat", addr);
        let body = format!("req-{}", i);
        handles.push(tokio::spawn(async move {
            let resp = reqwest::Client::new()
                .post(&url)
                .body(body.clone())
                .send()
                .await
                .unwrap();
            (body, resp.text().await.unwrap())
        }));
    }

    for h in handles {
        let (sent, got) = h.await.unwrap();
        assert!(
            got.ends_with(&sent),
            "concurrent request body must not mix up: sent `{sent}`, got `{got}`"
        );
    }

    home.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn cors_present_on_streaming_response() {
    // 浏览器跨域流式调用（OpenAI 风格）真实场景：流式响应也必须带 CORS 头。
    let (addr, sh) = start_server().await;
    let (local, lh) = start_local_mock().await;

    let cfg = DeploymentConfig {
        backend: BackendConfig {
            backend: Backend::Static { url: local.clone() },
            health_timeout_secs: 5,
        },
        rrclient: RrClientConfig {
            server_base: format!("http://{}", addr),
            name: "home".into(),
            token: "secret".into(),
        },
        models: vec![],
        info_port: None,
    };
    let deployer = Deployer::new(cfg);
    let deployed = deployer.deploy().await.unwrap();
    let client_cfg = deployer.build_client_config(&deployed.local_url);
    let home = tokio::spawn(run_client(client_cfg));
    wait_for_tunnel_connected(&addr).await;

    let big = "y".repeat(50_000);
    let resp = reqwest::Client::new()
        .post(format!("http://{}/t/home/v1/chat", addr))
        .header("Content-Type", "application/json")
        .header("Origin", "https://example.com")
        .body(big.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    assert_eq!(
        resp.headers().get("access-control-allow-origin").unwrap(),
        "*"
    );
    assert_eq!(resp.text().await.unwrap(), big);

    home.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn streaming_response_preserves_non_2xx_status() {
    // 本地返回 500（且 body 较大触发流式分片），验证流式路径正确透传非 2xx 状态码与 body，
    // 而非被误判为「完整响应到达」返回 502。
    let (addr, sh) = start_server().await;
    let (local, lh) = start_local_mock_status(500).await;

    let cfg = DeploymentConfig {
        backend: BackendConfig {
            backend: Backend::Static { url: local.clone() },
            health_timeout_secs: 5,
        },
        rrclient: RrClientConfig {
            server_base: format!("http://{}", addr),
            name: "home".into(),
            token: "secret".into(),
        },
        models: vec![],
        info_port: None,
    };
    let deployer = Deployer::new(cfg);
    let deployed = deployer.deploy().await.unwrap();
    let client_cfg = deployer.build_client_config(&deployed.local_url);
    let home = tokio::spawn(run_client(client_cfg));

    // 本地对所有路径固定返回 500，故轮询到 500 即表示隧道已连
    let client = reqwest::Client::new();
    for _ in 0..100 {
        if let Ok(r) = client
            .get(format!("http://{}/t/home/probe", addr))
            .send()
            .await
        {
            if r.status() == 500 {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let big = "z".repeat(50_000);
    let resp = client
        .post(format!("http://{}/t/home/v1/chat", addr))
        .header("Content-Type", "application/json")
        .body(big.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 500);
    assert_eq!(resp.text().await.unwrap(), big);

    home.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn skill_gate_state_mismatch_returns_409() {
    // 闸门 required_state 与引擎当前状态不符 → 409，且请求根本不会进入隧道。
    let rule = SkillRule {
        name: "fire".into(),
        cooldown: Duration::from_secs(60),
        cost: 1,
        required_state: Some("running".into()),
    };
    let (addr, sh) = start_skilled_server(rule).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/t/home/x", addr))
        .header("X-Skill", "fire")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    sh.abort();
}

#[tokio::test]
async fn skill_gate_insufficient_resource_returns_402() {
    // 技能消耗超过预算（cost 2_000_000 > budget 1_000_000）→ 402 Payment Required。
    let rule = SkillRule {
        name: "fire".into(),
        cooldown: Duration::from_secs(60),
        cost: 2_000_000,
        required_state: Some("idle".into()),
    };
    let (addr, sh) = start_skilled_server(rule).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/t/home/x", addr))
        .header("X-Skill", "fire")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYMENT_REQUIRED);
    sh.abort();
}

#[tokio::test]
async fn skill_gate_cooldown_isolated_per_skill() {
    // 多技能：fire 与 water 冷却相互独立。fire 冷却后再次请求被拦（429），
    // 但 water 首请求仍应放行（201，且确实穿透隧道到达本地）。
    let rules = vec![
        SkillRule {
            name: "fire".into(),
            cooldown: Duration::from_secs(60),
            cost: 1,
            required_state: Some("idle".into()),
        },
        SkillRule {
            name: "water".into(),
            cooldown: Duration::from_secs(60),
            cost: 1,
            required_state: Some("idle".into()),
        },
    ];
    let (addr, sh) = start_skilled_server_multi(rules).await;
    let (local, lh) = start_local_mock().await;

    let reg = register(&addr, "home", "secret").await;
    let ws_url = reg.json::<serde_json::Value>().await.unwrap()["ws_url"]
        .as_str()
        .unwrap()
        .to_string();
    let home = tokio::spawn(run_home(ws_url, local));
    wait_for_tunnel_connected(&addr).await;

    let client = reqwest::Client::new();
    let base = format!("http://{}/t/home/v1/chat", addr);

    // fire 首请求：闸门放行 + 穿透隧道到本地
    let fire1 = client
        .post(&base)
        .header("X-Skill", "fire")
        .body("a")
        .send()
        .await
        .unwrap();
    assert_eq!(fire1.status(), 201);

    // water 首请求：独立冷却，未受影响，仍放行并穿透
    let water1 = client
        .post(&base)
        .header("X-Skill", "water")
        .body("b")
        .send()
        .await
        .unwrap();
    assert_eq!(water1.status(), 201);

    // fire 再次：冷却中 → 429
    let fire2 = client
        .post(&base)
        .header("X-Skill", "fire")
        .body("a")
        .send()
        .await
        .unwrap();
    assert_eq!(fire2.status(), StatusCode::TOO_MANY_REQUESTS);

    // water 再次：此时也进入冷却 → 429
    let water2 = client
        .post(&base)
        .header("X-Skill", "water")
        .body("b")
        .send()
        .await
        .unwrap();
    assert_eq!(water2.status(), StatusCode::TOO_MANY_REQUESTS);

    home.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn cors_preflight_options_returns_204_with_full_headers() {
    // 浏览器跨域预检：OPTIONS 应直接 204，并带全套 CORS 头（含 max-age 减少预检频率）。
    let (addr, sh) = start_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .request(
            reqwest::Method::OPTIONS,
            format!("http://{}/t/home/v1/chat", addr),
        )
        .header("Origin", "https://example.com")
        .header("Access-Control-Request-Method", "POST")
        .header(
            "Access-Control-Request-Headers",
            "authorization,content-type",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let h = resp.headers();
    assert_eq!(h.get("access-control-allow-origin").unwrap(), "*");
    assert_eq!(
        h.get("access-control-allow-methods").unwrap(),
        "GET, POST, PUT, DELETE, PATCH, OPTIONS"
    );
    assert_eq!(h.get("access-control-allow-headers").unwrap(), "*");
    assert_eq!(h.get("access-control-max-age").unwrap(), "86400");
    sh.abort();
}

#[tokio::test]
async fn tunnel_reconnect_after_disconnect_restores_service() {
    // 家庭端断线后重新注册+连接，服务应恢复——模拟 run_client 的重连循环语义。
    let (addr, sh) = start_server().await;
    let (local, lh) = start_local_mock().await;

    // 第一次连接
    let reg = register(&addr, "home", "secret").await;
    let ws_url = reg.json::<serde_json::Value>().await.unwrap()["ws_url"]
        .as_str()
        .unwrap()
        .to_string();
    let home1 = tokio::spawn(run_home(ws_url, local.clone()));
    wait_for_tunnel_connected(&addr).await;

    let client = reqwest::Client::new();
    let url = format!("http://{}/t/home/v1/chat", addr);
    let r1 = client.post(&url).body("first").send().await.unwrap();
    assert_eq!(r1.status(), 201);

    // 断开家庭端，等云端感知到隧道移除（返回 tunnel not connected 的 502）。
    // 探测必须带短超时：若请求恰好赶在云端感知断连之前进入隧道，家庭端已死不会回包，
    // 无超时会被服务端 600s 代理超时拖满（曾使本测试耗时 600s）。超时即视为未就绪，继续轮询。
    home1.abort();
    let mut gone = false;
    for _ in 0..100 {
        if let Ok(r) = client
            .get(format!("http://{}/t/home/x", addr))
            .timeout(Duration::from_secs(1))
            .send()
            .await
        {
            if r.status() == 502 {
                let body = r.text().await.unwrap_or_default();
                if body.contains("tunnel not connected") {
                    gone = true;
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(gone, "server should notice tunnel disconnect");

    // 重连：重新注册拿新 ws_url，再次建立隧道后服务恢复
    let reg2 = register(&addr, "home", "secret").await;
    let ws_url2 = reg2.json::<serde_json::Value>().await.unwrap()["ws_url"]
        .as_str()
        .unwrap()
        .to_string();
    let home2 = tokio::spawn(run_home(ws_url2, local));
    wait_for_tunnel_connected(&addr).await;

    let r2 = client.post(&url).body("second").send().await.unwrap();
    assert_eq!(r2.status(), 201);
    assert!(r2.text().await.unwrap().ends_with("second"));

    home2.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn new_tunnel_connection_replaces_old_without_being_dropped() {
    // 同名隧道重复连接（如家庭端网络半开后重连，旧 WS 尚未被云端感知断开）：
    // 新连接注册后应立即接管；随后旧连接退出时的清理**不得误删新隧道**。
    let (addr, sh) = start_server().await;
    let (local_old, lh_old) = start_local_mock().await; // 旧连接指向的本地（201 echo）
    let (local_new, lh_new) = start_local_mock_status(207).await; // 新连接指向 207，可区分

    // 旧连接
    let reg = register(&addr, "home", "secret").await;
    let ws_url = reg.json::<serde_json::Value>().await.unwrap()["ws_url"]
        .as_str()
        .unwrap()
        .to_string();
    let home_old = tokio::spawn(run_home(ws_url.clone(), local_old));
    wait_for_tunnel_connected(&addr).await;

    // 新连接（旧的仍存活），注册表 insert 直接替换 → 请求应路由到新本地（207）
    let home_new = tokio::spawn(run_home(ws_url, local_new));
    let client = reqwest::Client::new();
    let url = format!("http://{}/t/home/v1/chat", addr);
    let mut took_over = false;
    for _ in 0..100 {
        if let Ok(r) = client.post(&url).body("x").send().await {
            if r.status() == 207 {
                took_over = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(took_over, "new tunnel connection should take over routing");

    // 关键断言：旧连接退出后，其清理不得移除新隧道——服务必须仍然可用（207）。
    home_old.abort();
    tokio::time::sleep(Duration::from_millis(300)).await; // 给云端时间处理旧连接关闭
    let r = client.post(&url).body("y").send().await.unwrap();
    assert_eq!(
        r.status(),
        207,
        "old connection cleanup must not drop the new tunnel"
    );

    home_new.abort();
    sh.abort();
    lh_old.abort();
    lh_new.abort();
}

// ───────────── 注册 · 心跳 · 探活 端到端 ─────────────

/// 启动一个「延迟 `delay` 后才响应」的本地 mock（模拟慢推理）。
async fn start_local_slow_mock(delay: Duration) -> (String, JoinHandle<()>) {
    let app = Router::new().fallback(any(move |body: Bytes| async move {
        tokio::time::sleep(delay).await;
        Response::builder()
            .status(200)
            .body(Body::from(body))
            .unwrap()
    }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{}", addr), handle)
}

/// 启动一个「永不返回」的本地 mock（模拟服务卡死）。
async fn start_local_hanging_mock() -> (String, JoinHandle<()>) {
    async fn hang() -> Response {
        tokio::time::sleep(Duration::from_secs(300)).await;
        Response::builder().status(200).body(Body::empty()).unwrap()
    }
    let app = Router::new().fallback(any(hang));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{}", addr), handle)
}

/// 注册并连上隧道，返回 (ws_url 所属注册信息, 家庭端句柄)。
async fn register_and_connect(addr: &str, local: String, alive: bool) -> JoinHandle<()> {
    let reg = register(addr, "home", "secret").await;
    let ws_url = reg.json::<serde_json::Value>().await.unwrap()["ws_url"]
        .as_str()
        .unwrap()
        .to_string();
    let home = tokio::spawn(run_home_with_heartbeat(ws_url, local, alive));
    wait_for_tunnel_connected(addr).await;
    home
}

#[tokio::test]
async fn register_issues_hash_code_and_heartbeat_accepts_it() {
    let (addr, sh) = start_server().await;
    let client = reqwest::Client::new();

    let v: serde_json::Value = client
        .post(format!("http://{}/api/register", addr))
        .json(&json!({"name": "home", "token": "secret"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let hash = v["hash_code"].as_str().expect("hash_code 应存在");
    assert_eq!(hash.len(), 16, "hash code 应为 16 位");
    assert_eq!(
        v["heartbeat_interval_millis"], 1_800_000,
        "默认心跳周期 30 分钟（毫秒下发）"
    );
    // 秒级字段已移除：周期统一由毫秒字段下发
    assert!(v.get("heartbeat_interval_secs").is_none());

    // 用 hash code 上报心跳
    let hb: serde_json::Value = client
        .post(format!("http://{}/api/heartbeat", addr))
        .json(&json!({"name": "home", "hash": hash}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(hb["status"], "ok");
    assert_eq!(hb["name"], "home");

    // 未知 hash：说明注册已被回收，服务应重新注册
    let r = client
        .post(format!("http://{}/api/heartbeat", addr))
        .json(&json!({"name": "home", "hash": "0000000000000000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);

    // hash 有效但 name 与注册名不符 → 同样视为未知注册
    let r = client
        .post(format!("http://{}/api/heartbeat", addr))
        .json(&json!({"name": "other", "hash": hash}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);

    sh.abort();
}

#[tokio::test]
async fn reaper_drops_registration_when_probe_reports_dead() {
    // 压缩时间：100ms 没心跳即探活，探活回应 alive=false → 注销注册并关闭隧道
    let health = HealthConfig {
        heartbeat_timeout: Duration::from_millis(100),
        probe_timeout: Duration::from_millis(500),
        reaper_interval: Duration::from_millis(50),
        first_response_timeout: Duration::from_millis(200),
        request_timeout: Duration::from_secs(3),
        ..Default::default()
    };
    let (addr, sh) = start_server_with_health(health).await;
    let (local, lh) = start_local_mock().await;
    let home = register_and_connect(&addr, local, false).await;

    let client = reqwest::Client::new();
    let mut dropped = false;
    for _ in 0..100 {
        let v: serde_json::Value = client
            .get(format!("http://{}/api/services", addr))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if v["services"].as_array().unwrap().is_empty() {
            dropped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(dropped, "探活失败的服务应被注销注册");

    // 隧道通道也应被关闭：后续请求不再路由到该服务
    let resp = client
        .get(format!("http://{}/t/home/x", addr))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);

    home.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn reaper_keeps_registration_when_probe_succeeds() {
    // 心跳缺失但服务确实在运行（探活成功）→ 保留注册
    let health = HealthConfig {
        heartbeat_timeout: Duration::from_millis(60),
        probe_timeout: Duration::from_millis(500),
        reaper_interval: Duration::from_millis(50),
        first_response_timeout: Duration::from_millis(200),
        request_timeout: Duration::from_secs(3),
        ..Default::default()
    };
    let (addr, sh) = start_server_with_health(health).await;
    let (local, lh) = start_local_mock().await;
    let home = register_and_connect(&addr, local, true).await;

    // 静默阈值远小于下面的观察时长：期间必然被扫描到并探活成功
    tokio::time::sleep(Duration::from_millis(600)).await;
    let v: serde_json::Value = reqwest::Client::new()
        .get(format!("http://{}/api/services", addr))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        v["services"].as_array().unwrap().len(),
        1,
        "探活成功的服务不应被注销"
    );

    home.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn heartbeat_reports_keep_registration_fresh() {
    // 服务按周期上报心跳 → 注册始终不过期（压扁到 60ms 周期 / 200ms 阈值）
    let health = HealthConfig {
        heartbeat_timeout: Duration::from_millis(200),
        probe_timeout: Duration::from_millis(300),
        reaper_interval: Duration::from_millis(50),
        heartbeat_interval: Duration::from_millis(60),
        ..Default::default()
    };
    let (addr, sh) = start_server_with_health(health).await;
    let reg = register(&addr, "home", "secret").await;
    let v: serde_json::Value = reg.json().await.unwrap();
    let hash = v["hash_code"].as_str().unwrap().to_string();
    assert_eq!(
        v["heartbeat_interval_millis"], 60,
        "服务端下发的周期应随配置（含毫秒精度）"
    );

    let url = format!("http://{}/api/heartbeat", addr);
    let hash_clone = hash.clone();
    let beater = tokio::spawn(async move {
        let client = reqwest::Client::new();
        loop {
            tokio::time::sleep(Duration::from_millis(60)).await;
            let _ = client
                .post(&url)
                .json(&json!({"name": "home", "hash": hash_clone}))
                .send()
                .await;
        }
    });

    tokio::time::sleep(Duration::from_millis(700)).await;
    let listed: serde_json::Value = reqwest::Client::new()
        .get(format!("http://{}/api/services", addr))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let list = listed["services"].as_array().unwrap();
    assert_eq!(list.len(), 1, "持续心跳的服务不应被回收");
    assert_eq!(list[0]["stale"], false);

    beater.abort();
    sh.abort();
}

#[tokio::test]
async fn forwarding_probes_service_after_first_response_timeout() {
    // 本地推理需要 1.2s，云端首响窗口只有 300ms：
    // 每 300ms 探活一次，服务回应 alive → 继续等待，最终拿到完整响应。
    let health = HealthConfig {
        first_response_timeout: Duration::from_millis(300),
        probe_timeout: Duration::from_millis(500),
        request_timeout: Duration::from_secs(10),
        heartbeat_timeout: Duration::from_secs(3600),
        reaper_interval: Duration::from_secs(3600),
        ..Default::default()
    };
    let (addr, sh) = start_server_with_health(health).await;
    let (local, lh) = start_local_slow_mock(Duration::from_millis(1200)).await;
    let home = register_and_connect(&addr, local, true).await;

    let started = std::time::Instant::now();
    let resp = reqwest::Client::new()
        .post(format!("http://{}/t/home/v1/chat", addr))
        .timeout(Duration::from_secs(10))
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), "hello");
    assert!(
        started.elapsed() >= Duration::from_millis(1200),
        "应真的等到本地响应，而不是被 1 分钟窗口掐断"
    );

    home.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn forwarding_aborts_when_service_stops_answering_probes() {
    // 本地服务卡死（永不返回）且探活回 alive=false → 云端应主动放弃并返回 504
    let health = HealthConfig {
        first_response_timeout: Duration::from_millis(300),
        probe_timeout: Duration::from_millis(500),
        request_timeout: Duration::from_secs(5),
        heartbeat_timeout: Duration::from_secs(3600),
        reaper_interval: Duration::from_secs(3600),
        ..Default::default()
    };
    let (addr, sh) = start_server_with_health(health).await;
    let (local, lh) = start_local_hanging_mock().await;
    let reg = register(&addr, "home", "secret").await;
    let ws_url = reg.json::<serde_json::Value>().await.unwrap()["ws_url"]
        .as_str()
        .unwrap()
        .to_string();
    // 探活回 alive=false 的家庭端（本地卡死）
    let home = tokio::spawn(run_home_with_heartbeat(ws_url, local, false));
    tokio::time::sleep(Duration::from_millis(200)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{}/t/home/v1/chat", addr))
        .timeout(Duration::from_secs(10))
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::GATEWAY_TIMEOUT);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("heartbeat"),
        "应说明放弃原因是探活失败: {body}"
    );

    home.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn client_sends_heartbeat_and_answers_probes() {
    // 真实 client（run_client）端到端：注册拿 hash → 按周期心跳 → 云端探活得到回应。
    let health = HealthConfig {
        heartbeat_interval: Duration::from_millis(60),
        heartbeat_timeout: Duration::from_millis(120),
        probe_timeout: Duration::from_millis(500),
        reaper_interval: Duration::from_millis(50),
        first_response_timeout: Duration::from_millis(200),
        request_timeout: Duration::from_secs(5),
    };
    let (addr, sh) = start_server_with_health(health).await;
    let (local, lh) = start_local_mock().await;

    let cfg = rrserver::client::ClientConfig {
        server_base: format!("http://{}", addr),
        name: "home".into(),
        token: "secret".into(),
        local_url: local,
    };
    let home = tokio::spawn(run_client(cfg));
    wait_for_tunnel_connected(&addr).await;

    // 客户端持续心跳（60ms 周期 < 120ms 阈值），注册应长期存活
    tokio::time::sleep(Duration::from_millis(700)).await;
    let listed: serde_json::Value = reqwest::Client::new()
        .get(format!("http://{}/api/services", addr))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let list = listed["services"].as_array().unwrap();
    assert_eq!(list.len(), 1, "真实 client 的心跳应保住注册");
    assert_eq!(list[0]["stale"], false);
    // 心跳确实被记录（距上次心跳的时间应远小于观察时长）
    assert!(list[0]["heartbeat_age_secs"].as_u64().unwrap() < 1);

    home.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn client_reconnects_with_new_hash_after_registration_is_closed() {
    // 云端关闭注册（连同隧道）后，真实 client 应自动重连并重新注册 —— 拿到**新的** hash code。
    // 这是 Python e2e 里「注销后 client 自动恢复」断言的 Rust 侧守护（CI 必跑）。
    let health = HealthConfig {
        heartbeat_interval: Duration::from_millis(200),
        heartbeat_timeout: Duration::from_secs(3600),
        probe_timeout: Duration::from_millis(500),
        reaper_interval: Duration::from_secs(3600),
        first_response_timeout: Duration::from_millis(300),
        request_timeout: Duration::from_secs(5),
    };
    let (addr, sh) = start_server_with_health(health).await;
    let (local, lh) = start_local_mock().await;

    let cfg = rrserver::client::ClientConfig {
        server_base: format!("http://{}", addr),
        name: "home".into(),
        token: "secret".into(),
        local_url: local,
    };
    let home = tokio::spawn(run_client(cfg));
    wait_for_tunnel_connected(&addr).await;

    let client = reqwest::Client::new();
    let listed: serde_json::Value = client
        .get(format!("http://{}/api/services", addr))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let old_hash = listed["services"][0]["hash"]
        .as_str()
        .expect("client 应已注册")
        .to_string();

    let closed = client
        .post(format!("http://{}/api/unregister", addr))
        .json(&json!({"name": "home", "hash": old_hash}))
        .send()
        .await
        .unwrap();
    assert_eq!(closed.status(), StatusCode::OK);
    // 旧 hash 立即失效
    let stale_hb = client
        .post(format!("http://{}/api/heartbeat", addr))
        .json(&json!({"name": "home", "hash": old_hash}))
        .send()
        .await
        .unwrap();
    assert_eq!(stale_hb.status(), StatusCode::NOT_FOUND);

    // client 重连并重新注册：应出现一个不同于旧值的 hash
    let mut new_hash = None;
    for _ in 0..200 {
        let v: serde_json::Value = client
            .get(format!("http://{}/api/services", addr))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if let Some(h) = v["services"][0].get("hash").and_then(|x| x.as_str()) {
            if h != old_hash {
                new_hash = Some(h.to_string());
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let new_hash = new_hash.expect("client 应重新注册并拿到新的 hash code");
    // 隧道随之恢复，且新 hash 可用于心跳
    wait_for_tunnel_connected(&addr).await;
    let hb = client
        .post(format!("http://{}/api/heartbeat", addr))
        .json(&json!({"name": "home", "hash": new_hash}))
        .send()
        .await
        .unwrap();
    assert_eq!(hb.status(), StatusCode::OK);

    home.abort();
    sh.abort();
    lh.abort();
}

#[tokio::test]
async fn forwarding_without_registration_is_rejected() {
    // 隧道在、注册没了（例如注册被回收而隧道尚未断开）：无从探活，
    // 应明确拒绝并提示服务重新注册，而不是无限等待。
    let health = HealthConfig {
        first_response_timeout: Duration::from_millis(200),
        probe_timeout: Duration::from_millis(500),
        request_timeout: Duration::from_secs(5),
        heartbeat_timeout: Duration::from_secs(3600),
        reaper_interval: Duration::from_secs(3600),
        ..Default::default()
    };
    let (addr, state, sh) = start_server_with_state(health).await;
    // 本地 1.2s 才回：足以触发首响探活（200ms），又不会让恢复后的断言等太久
    let (local, lh) = start_local_slow_mock(Duration::from_millis(1200)).await;

    let reg = register(&addr, "home", "secret").await;
    let v: serde_json::Value = reg.json().await.unwrap();
    let hash = v["hash_code"].as_str().unwrap().to_string();
    let ws_url = v["ws_url"].as_str().unwrap().to_string();
    let home = tokio::spawn(run_home(ws_url, local));
    wait_for_tunnel_connected(&addr).await;

    // 仅抹掉注册记录，隧道通道保持连接
    assert!(state.services.remove_by_hash(&hash).await.is_some());

    let resp = reqwest::Client::new()
        .post(format!("http://{}/t/home/v1/chat", addr))
        .timeout(Duration::from_secs(10))
        .body("hello")
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT, "body={body}");
    assert!(body.contains("not registered"), "应提示先重新注册: {body}");

    // 重新注册后服务恢复：注册记录重新出现，转发正常
    let reg2 = register(&addr, "home", "secret").await;
    assert_eq!(reg2.status(), StatusCode::OK);
    let resp2 = reqwest::Client::new()
        .post(format!("http://{}/t/home/v1/chat", addr))
        .timeout(Duration::from_secs(10))
        .body("again")
        .send()
        .await
        .unwrap();
    let status2 = resp2.status();
    let body2 = resp2.text().await.unwrap_or_default();
    // 慢 mock 固定返回 200 并回显 body
    assert_eq!(status2, StatusCode::OK, "body={body2}");
    assert_eq!(body2, "again");

    home.abort();
    sh.abort();
    lh.abort();
}
