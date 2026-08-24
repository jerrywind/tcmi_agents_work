//! 端到端拉通示例：云端 server + 本地 mock 服务 + 家庭端 client + 外部请求。
//!
//! 验证完整链路：外部请求 → 云端 rrserver → WebSocket 隧道 → 家庭端 → 本地服务 → 原路回传。
//! 包含：普通请求穿透、query 透传，以及「真·流式」透传（本地分片、间隔产出，云端边收边发）。
//! 运行：`cargo run --example e2e --offline`

use axum::{
    body::{Body, Bytes},
    http::{Method, Uri},
    response::Response,
    routing::any,
    Router,
};
use futures::StreamExt;
use rrserver::client::{run_client, ClientConfig};
use rrserver::server::{run_server, AppState, TunnelAuth};
use rrserver::state::Registry;

#[tokio::main]
async fn main() {
    // 避免本机代理（如 7897）拦截 127.0.0.1 的直连
    for k in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
        std::env::remove_var(k);
    }
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");

    // 1) 本地 mock 服务（模拟家庭网络中的本地 llm 服务），回显 method|path|query + body
    tokio::spawn(local_mock());

    // 2) 云端中继 server：监听 18080，隧道名 home / token secret
    let auth = TunnelAuth::from_list(&[("home".into(), "secret".into())]);
    let state = AppState {
        registry: Registry::new(),
        auth,
        external_ws_base: "ws://127.0.0.1:18080".into(),
        skills: None,
    };
    tokio::spawn(async move {
        let _ = run_server("127.0.0.1:18080".into(), state).await;
    });

    // 3) 家庭端 client：注册到云端并把请求转发到本地 19090
    let cfg = ClientConfig {
        server_base: "http://127.0.0.1:18080".into(),
        name: "home".into(),
        token: "secret".into(),
        local_url: "http://127.0.0.1:19090".into(),
    };
    tokio::spawn(run_client(cfg));

    // 4) 等待隧道真正就绪（注册 + WebSocket 建立完成），避免固定 sleep 的竞态
    wait_ready().await;

    let client = reqwest::Client::new();
    let mut all_ok = true;

    // 5) 外部请求一：POST 携带 query 与 body，应无损穿透隧道到达本地
    let resp = client
        .post("http://127.0.0.1:18080/t/home/v1/chat?model=gpt")
        .body("hello local")
        .send()
        .await
        .expect("request failed");
    let status = resp.status();
    let body = resp.text().await.unwrap();
    let pass1 = status == 200
        && body.contains("POST|/v1/chat|model=gpt")
        && body.contains("hello local");
    println!("[1] POST /t/home/v1/chat?model=gpt -> {status}");
    println!("    body = {body}");
    println!("    expect POST|/v1/chat|model=gpt + hello local => {pass1}");
    all_ok &= pass1;

    // 6) 外部请求二：GET 携带 query，验证 query 串完整透传
    let resp = client
        .get("http://127.0.0.1:18080/t/home/status?id=7")
        .send()
        .await
        .expect("request failed");
    let status = resp.status();
    let body = resp.text().await.unwrap();
    let pass2 = status == 200 && body.contains("GET|/status|id=7");
    println!("[2] GET /t/home/status?id=7 -> {status}");
    println!("    body = {body}");
    println!("    expect GET|/status|id=7 => {pass2}");
    all_ok &= pass2;

    // 7) 外部请求三：本地 mock 分 5 片、每片间隔 20ms 产出，验证云端「真·流式」透传
    let resp = client
        .get("http://127.0.0.1:18080/t/home/stream")
        .send()
        .await
        .expect("stream request failed");
    let status = resp.status();
    let body = resp.text().await.unwrap();
    let pass3 = status == 200 && body == "chunk-0chunk-1chunk-2chunk-3chunk-4";
    println!("[3] GET /t/home/stream -> {status}");
    println!("    body = {body}");
    println!("    expect 5 streamed chunks concatenated => {pass3}");
    all_ok &= pass3;

    if all_ok {
        println!("\nE2E PASS: 云端 <-> 隧道 <-> 本地链路已打通（含真·流式透传）");
        std::process::exit(0);
    } else {
        println!("\nE2E FAIL");
        std::process::exit(1);
    }
}

/// 轮询直到家庭端隧道真正就绪：本地 mock 始终在 19090，故隧道连上后 `/t/home/x` 立即返回 200。
/// 用就绪轮询替代固定 sleep，避免慢机 / CI 上的竞态。
async fn wait_ready() {
    let client = reqwest::Client::new();
    for _ in 0..100 {
        if let Ok(resp) = client
            .get("http://127.0.0.1:18080/t/home/x")
            .send()
            .await
        {
            if resp.status() == 200 {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("tunnel did not become ready in time");
}

/// 本地 mock：回显 `METHOD|PATH|QUERY` + 请求体；对 `/stream` 分片、间隔产出以验证真·流式透传。
async fn local_mock() {
    async fn echo(method: Method, uri: Uri, body: Bytes) -> Response {
        if uri.path() == "/stream" {
            // 分 5 片、每片间隔 20ms 产出，验证云端「真·流式」透传（边生成边发，而非攒完再发）。
            let s = futures::stream::unfold(0u8, |i| async move {
                if i < 5 {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    Some((Bytes::from(format!("chunk-{}", i)), i + 1))
                } else {
                    None
                }
            })
            .map(|b| Ok::<_, std::io::Error>(b));
            return Response::builder()
                .status(200)
                .header("Content-Type", "text/plain")
                .body(Body::from_stream(s))
                .unwrap();
        }
        let payload = format!("{}|{}|{}", method, uri.path(), uri.query().unwrap_or(""));
        let mut out = payload.into_bytes();
        out.extend_from_slice(&body);
        Response::builder()
            .status(200)
            .header("Content-Type", "text/plain")
            .body(Body::from(out))
            .unwrap()
    }
    let app = Router::new().fallback(any(echo));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:19090").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
