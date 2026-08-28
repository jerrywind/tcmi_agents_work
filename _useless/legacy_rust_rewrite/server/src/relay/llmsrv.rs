//! 家庭端 llm_server 部署 + 注册包装（与 rrserver/src/llmsrv.rs 一致）。
//!
//! 负责：启动本地 LM Studio 兼容服务（或从远端拉取部署），然后以家庭端 client 身份
//! 向云端 server 注册隧道，使云端能经 `/t/<name>/v1` 访问本地模型。
//! 阶段 A 保留原逻辑，供后续 server 经隧道调 LLM 使用。

use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use super::client::{ClientConfig, run_client};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmServerConfig {
    pub rrclient: RrClientConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub deploy: DeployConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RrClientConfig {
    pub server: String,
    pub name: String,
    pub token: String,
    #[serde(default = "default_local")]
    pub local: String,
    #[serde(default = "default_reconnect")]
    pub reconnect_secs: u64,
}

fn default_local() -> String {
    "http://127.0.0.1:8080".to_string()
}
fn default_reconnect() -> u64 {
    5
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LlmConfig {
    #[serde(default = "default_lm_base")]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_text_model")]
    pub text_model: String,
    #[serde(default = "default_vision_model")]
    pub vision_model: String,
}

fn default_lm_base() -> String {
    "http://localhost:11223/v1".to_string()
}
fn default_text_model() -> String {
    "google/gemma-4-12b-qat".to_string()
}
fn default_vision_model() -> String {
    "vl-gemma".to_string()
}

/// 部署配置：如何拉起本地模型服务。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeployConfig {
    /// 启动命令（如 lmstudio-cli 或 python -m ...）；为空则不自动拉起。
    #[serde(default)]
    pub cmd: Vec<String>,
    /// 拉起前等待就绪的 URL（健康检查）。
    #[serde(default)]
    pub health_url: String,
    /// 就绪超时（秒）。
    #[serde(default = "default_health_timeout")]
    pub health_timeout: u64,
}

fn default_health_timeout() -> u64 {
    60
}

impl LlmServerConfig {
    pub fn from_file(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let cfg: LlmServerConfig = serde_yaml_兼容::parse(&content)?;
        Ok(cfg)
    }
}

/// 启动：先按需部署本地模型服务，再运行家庭端 client（注册隧道）。
pub async fn run(cfg: LlmServerConfig) -> anyhow::Result<()> {
    if !cfg.deploy.cmd.is_empty() {
        deploy_local(&cfg.deploy).await?;
    }
    let client_cfg = ClientConfig {
        server: cfg.rrclient.server.clone(),
        name: cfg.rrclient.name.clone(),
        token: cfg.rrclient.token.clone(),
        local: cfg.rrclient.local.clone(),
        reconnect_secs: cfg.rrclient.reconnect_secs,
    };
    run_client(client_cfg).await
}

async fn deploy_local(d: &DeployConfig) -> anyhow::Result<()> {
    if d.cmd.is_empty() {
        return Ok(());
    }
    let (program, args) = d.cmd.split_first().unwrap();
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    // 简易就绪等待：轮询 health_url
    if !d.health_url.is_empty() {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(d.health_timeout);
        let client = reqwest::Client::new();
        loop {
            if let Ok(r) = client.get(&d.health_url).send().await {
                if r.status().is_success() {
                    break;
                }
            }
            if std::time::Instant::now() > deadline {
                anyhow::bail!("local model service not ready in time");
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }
    // 兜底等待子进程退出（实际常驻，这里仅做占位）
    let _ = child.try_wait();
    Ok(())
}

/// 占位：兼容 serde_yaml / toml 解析（阶段 A 用 serde_yaml 解析 llm_server.toml）。
/// 注：rrserver 的 llm_server.toml 为 TOML，这里用 toml 解析以保持一致。
mod serde_yaml_兼容 {
    use serde::de::DeserializeOwned;
    pub fn parse<T: DeserializeOwned>(s: &str) -> anyhow::Result<T> {
        Ok(toml::from_str(s)?)
    }
}
