//! harness 运行配置
//!
//! 配置来自（优先级从低到高）：
//! 1. `resources/config.yaml` 默认值
//! 2. 环境变量（HARNESS_*）
//! 3. 命令行参数
//!
//! 这样运维可调端口/地址，中医可改资源目录下的文案。

use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub struct HarnessConfig {
    /// 监听地址，如 0.0.0.0:8011
    #[serde(default = "default_listen")]
    pub listen: String,

    /// 上游 LLM 网关（lmstudio / llm_server）地址
    #[serde(default = "default_llm_base")]
    pub llm_base_url: String,

    /// LLM 网关 API Key（无则留空）
    #[serde(default)]
    pub llm_api_key: String,

    /// 默认模型名
    #[serde(default = "default_model")]
    pub model: String,

    /// 资源目录（YAML 所在）
    #[serde(default = "default_resources_dir")]
    pub resources_dir: PathBuf,

    /// 单次 LLM 调用超时（秒）
    #[serde(default = "default_timeout")]
    pub llm_timeout_secs: u64,

    /// 是否在每次请求时热重载 YAML（便于调试修改）
    #[serde(default)]
    pub hot_reload: bool,

    /// 可选 RAG 检索端点（留空则 tcm-rag 返回提示）
    #[serde(default)]
    pub rag_endpoint: Option<String>,

    /// 可选隧道配置：经 rrserver 暴露本服务（None 表示不启用隧道）
    #[serde(default)]
    pub tunnel: Option<TunnelConfig>,
}

/// 隧道配置：把 harness 通过 rrserver 中继服务器暴露到公网
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TunnelConfig {
    /// rrserver 云端中继地址（ws(s)://host:port）
    pub server: String,
    /// 隧道名称（公网访问路径 /t/<name>）
    pub name: String,
    /// 校验令牌（可选）
    #[serde(default)]
    pub token: Option<String>,
    /// 本地服务 URL（家庭端回连 harness 用）
    pub local_url: String,
}

fn default_listen() -> String { "0.0.0.0:8011".into() }
fn default_llm_base() -> String { "http://localhost:11223/v1".into() }
fn default_model() -> String { "google/gemma-4-12b-qat".into() }
fn default_resources_dir() -> PathBuf { PathBuf::from("resources") }
fn default_timeout() -> u64 { 120 }

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            llm_base_url: default_llm_base(),
            llm_api_key: String::new(),
            model: default_model(),
            resources_dir: default_resources_dir(),
            llm_timeout_secs: default_timeout(),
            hot_reload: false,
            rag_endpoint: None,
            tunnel: None,
        }
    }
}

/// 命令行参数（覆盖配置文件）
#[derive(Parser, Debug)]
#[command(name = "harness", about = "风蓝 TCM 后端 harness（Rust 复刻）")]
pub struct Cli {
    /// 配置文件路径
    #[arg(long, default_value = "resources/config.yaml")]
    pub config: PathBuf,

    /// 监听地址（覆盖配置）
    #[arg(long)]
    pub listen: Option<String>,

    /// 资源目录（覆盖配置）
    #[arg(long)]
    pub resources: Option<PathBuf>,

    /// 隧道中继服务器地址（ws(s)://host:port）；提供即启用 rrserver 隧道
    #[arg(long)]
    pub tunnel_server: Option<String>,

    /// 隧道名称（公网访问路径 /t/<name>）
    #[arg(long)]
    pub tunnel_name: Option<String>,

    /// 隧道校验令牌（可选）
    #[arg(long)]
    pub tunnel_token: Option<String>,
}

impl HarnessConfig {
    /// 从配置文件 + 环境变量 + 命令行合并出最终配置
    pub fn load(cli: &Cli) -> Result<Self> {
        let mut cfg = if cli.config.exists() {
            let text = std::fs::read_to_string(&cli.config)
                .with_context(|| format!("读取配置失败: {}", cli.config.display()))?;
            serde_yaml::from_str(&text).context("解析 config.yaml 失败")?
        } else {
            HarnessConfig::default()
        };

        if let Ok(v) = std::env::var("HARNESS_LISTEN") { cfg.listen = v; }
        if let Ok(v) = std::env::var("HARNESS_LLM_BASE_URL") { cfg.llm_base_url = v; }
        if let Ok(v) = std::env::var("HARNESS_LLM_API_KEY") { cfg.llm_api_key = v; }
        if let Ok(v) = std::env::var("HARNESS_MODEL") { cfg.model = v; }
        if let Ok(v) = std::env::var("HARNESS_RESOURCES_DIR") {
            cfg.resources_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("HARNESS_RAG_ENDPOINT") {
            cfg.rag_endpoint = Some(v);
        }

        if let Some(l) = &cli.listen { cfg.listen = l.clone(); }
        if let Some(r) = &cli.resources { cfg.resources_dir = r.clone(); }

        // 隧道配置：CLI 或环境变量提供 server+name 即启用
        let t_server = cli
            .tunnel_server
            .clone()
            .or_else(|| std::env::var("HARNESS_TUNNEL_SERVER").ok());
        let t_name = cli
            .tunnel_name
            .clone()
            .or_else(|| std::env::var("HARNESS_TUNNEL_NAME").ok());
        let t_token = cli
            .tunnel_token
            .clone()
            .or_else(|| std::env::var("HARNESS_TUNNEL_TOKEN").ok());
        if let (Some(server), Some(name)) = (t_server, t_name) {
            // local_url：把监听地址的 0.0.0.0 换成本机回环，供家庭端回连
            let local = cfg
                .listen
                .replace("0.0.0.0", "127.0.0.1")
                .replace("::", "127.0.0.1");
            cfg.tunnel = Some(TunnelConfig {
                server,
                name,
                token: t_token,
                local_url: format!("http://{local}"),
            });
        }

        Ok(cfg)
    }
}
