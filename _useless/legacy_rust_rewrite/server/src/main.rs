//! server 二进制入口：合并 rrserver 中继能力与 backend 诊断编排。
//!
//! 阶段 A：解析 `--listen` / `--config`，构建 AppState，启动 axum。
//! 后续阶段在 config 中增加 diagnose 相关字段并在此处串联 store/orchestrator。

use clap::Parser;
use server::api;

#[derive(Parser)]
#[command(name = "server", version, about = "TCM server: 诊断编排 + 反向隧道中继")]
struct Cli {
    /// 监听地址（容器内建议 0.0.0.0:8080）
    #[arg(long, default_value = "0.0.0.0:8080")]
    listen: String,
    /// 配置文件（TOML，含 [[tunnels]] 等）
    #[arg(long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    let router = api::build_router_from_config(cli.config.as_deref())?;
    let listener = tokio::net::TcpListener::bind(&cli.listen).await?;
    tracing::info!("server listening on {}", cli.listen);
    axum::serve(listener, router).await?;
    Ok(())
}
