//! harness 可执行入口

use anyhow::Context;
use clap::Parser;
use harness::config::Cli;
use harness::http;
use harness::AppState;
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,harness=debug".into()),
        )
        .init();

    let cli = Cli::parse();
    let config = harness::config::HarnessConfig::load(&cli)?;
    let state = AppState::load(config).await?;

    // 隧道：若配置了 rrserver 中继，则在后台启动家庭端 client，使本服务经隧道暴露
    if let Some(t) = &state.config.tunnel {
        let server_base = t
            .server
            .replace("wss://", "https://")
            .replace("ws://", "http://");
        let client_cfg = rrserver::client::ClientConfig {
            server_base,
            name: t.name.clone(),
            token: t.token.clone().unwrap_or_default(),
            local_url: t.local_url.clone(),
        };
        tracing::info!(
            "启用 rrserver 隧道：{} (本地回连 {})",
            t.server,
            t.local_url
        );
        tokio::spawn(async move {
            if let Err(e) = rrserver::client::run_client(client_cfg).await {
                tracing::error!("隧道运行失败: {e:#}");
            }
        });
    }

    let addr: SocketAddr = state.config.listen.parse().context("listen 地址格式错误")?;
    tracing::info!("harness 监听于 {addr}");

    let app = http::build_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
