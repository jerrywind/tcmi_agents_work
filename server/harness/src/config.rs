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

    /// 工具调用最大轮数（T2.2）
    ///
    /// 模型可在工具结果之上继续追问/查证：每轮 = 一次带工具的 LLM 调用。
    /// 达到上限后不再给工具，改为一次纯汇总调用以拿到最终文本。
    /// 设为 1 即退化为此前的「1 次带工具 + 1 次汇总」。
    #[serde(default = "default_max_tool_rounds")]
    pub max_tool_rounds: usize,

    /// LLM 调用失败后的重试次数（T3.5，0 表示不重试）
    ///
    /// 仅对**可重试**失败生效：超时、连接失败、5xx、429。
    /// 4xx 语义错误（如模型不存在、参数非法）直接失败，不做无谓重试。
    #[serde(default = "default_llm_max_retries")]
    pub llm_max_retries: u32,

    /// 重试退避基数（毫秒）：第 n 次重试等待 `base * 2^n`（上限 16 倍）
    #[serde(default = "default_retry_backoff_ms")]
    pub llm_retry_backoff_ms: u64,

    /// 可选 RAG 检索端点（留空则 tcm-rag 返回提示）
    #[serde(default)]
    pub rag_endpoint: Option<String>,

    /// 外部 MCP server 列表（T2.4）：启动时 `tools/list` 后挂载为 `mcp__<client>__<tool>`
    #[serde(default)]
    pub mcp_clients: Vec<McpClientConfig>,

    /// 是否在每次请求时热重载 YAML（便于调试修改）
    #[serde(default)]
    pub hot_reload: bool,

    /// 报告持久化目录（T5.1）
    ///
    /// `None` 表示不持久化（默认）：harness 保持无状态，`/chat` 不返回 `report_id`。
    /// 配置后每次 `/chat` 落盘一份报告，`GET /reports/:id` 可回查。
    #[serde(default)]
    pub store_dir: Option<PathBuf>,

    /// 落盘前是否脱敏（T5.4）：屏蔽手机号 / 身份证 / 邮箱 / 长数字串
    ///
    /// 默认开启。仅在运维明确接受「明文入库」风险时关闭。
    #[serde(default = "default_true")]
    pub store_redact: bool,

    /// `GET /reports` 返回条数上限
    #[serde(default = "default_store_list_limit")]
    pub store_list_limit: usize,

    /// 可选隧道配置：经 rrserver 暴露本服务（None 表示不启用隧道）
    #[serde(default)]
    pub tunnel: Option<TunnelConfig>,
}

/// 外部 MCP server 配置（T2.4）
///
/// 启动时对 `url` 发一次 `tools/list`，把返回的工具逐个挂成技能：
/// 显示名固定为 `mcp__<name>__<tool>`，便于 `GET /skills` 一眼区分外部来源。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct McpClientConfig {
    /// 客户端名（同时作为技能名前缀），如 `kb`
    pub name: String,
    /// MCP Streamable HTTP 端点，如 http://localhost:9000/mcp
    pub url: String,
    /// 工具白名单；留空表示挂载该 server 的全部工具
    #[serde(default)]
    pub tools: Vec<String>,
    /// 是否启用（false 可临时停用而不删配置）
    #[serde(default = "default_true")]
    pub enabled: bool,
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

fn default_listen() -> String {
    "0.0.0.0:8011".into()
}
fn default_llm_base() -> String {
    "http://localhost:11223/v1".into()
}
fn default_model() -> String {
    "google/gemma-4-12b-qat".into()
}
fn default_resources_dir() -> PathBuf {
    PathBuf::from("resources")
}
fn default_timeout() -> u64 {
    120
}
fn default_max_tool_rounds() -> usize {
    3
}
fn default_llm_max_retries() -> u32 {
    2
}
fn default_retry_backoff_ms() -> u64 {
    500
}
fn default_true() -> bool {
    true
}
fn default_store_list_limit() -> usize {
    crate::store::DEFAULT_LIST_LIMIT
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            llm_base_url: default_llm_base(),
            llm_api_key: String::new(),
            model: default_model(),
            resources_dir: default_resources_dir(),
            llm_timeout_secs: default_timeout(),
            max_tool_rounds: default_max_tool_rounds(),
            llm_max_retries: default_llm_max_retries(),
            llm_retry_backoff_ms: default_retry_backoff_ms(),
            rag_endpoint: None,
            mcp_clients: Vec::new(),
            hot_reload: false,
            tunnel: None,
            store_dir: None,
            store_redact: true,
            store_list_limit: crate::store::DEFAULT_LIST_LIMIT,
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

/// 解析 `HARNESS_MCP_CLIENTS`（容器/命令行场景不便改 YAML 时用）
///
/// 格式：`name=kb,url=http://host:9000/mcp;name=emr,url=http://host:9100/mcp`
/// 解析失败或缺少 name/url 的片段会被跳过并告警，不影响启动。
fn parse_mcp_clients(raw: &str) -> Vec<McpClientConfig> {
    let mut out = Vec::new();
    for seg in raw.split(';') {
        let seg = seg.trim();
        if seg.is_empty() {
            continue;
        }
        let mut name = None;
        let mut url = None;
        for kv in seg.split(',') {
            match kv.split_once('=') {
                Some(("name", v)) => name = Some(v.trim().to_string()),
                Some(("url", v)) => url = Some(v.trim().to_string()),
                _ => {}
            }
        }
        match (name, url) {
            (Some(n), Some(u)) if !n.is_empty() && !u.is_empty() => out.push(McpClientConfig {
                name: n,
                url: u,
                tools: Vec::new(),
                enabled: true,
            }),
            _ => tracing::warn!("HARNESS_MCP_CLIENTS 片段无法解析，已跳过：{seg}"),
        }
    }
    out
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

        if let Ok(v) = std::env::var("HARNESS_LISTEN") {
            cfg.listen = v;
        }
        if let Ok(v) = std::env::var("HARNESS_LLM_BASE_URL") {
            cfg.llm_base_url = v;
        }
        if let Ok(v) = std::env::var("HARNESS_LLM_API_KEY") {
            cfg.llm_api_key = v;
        }
        if let Ok(v) = std::env::var("HARNESS_MODEL") {
            cfg.model = v;
        }
        if let Ok(v) = std::env::var("HARNESS_RESOURCES_DIR") {
            cfg.resources_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("HARNESS_RAG_ENDPOINT") {
            cfg.rag_endpoint = Some(v);
        }
        if let Ok(v) = std::env::var("HARNESS_LLM_TIMEOUT_SECS") {
            if let Ok(n) = v.parse() {
                cfg.llm_timeout_secs = n;
            }
        }
        if let Ok(v) = std::env::var("HARNESS_MAX_TOOL_ROUNDS") {
            if let Ok(n) = v.parse() {
                cfg.max_tool_rounds = n;
            }
        }
        if let Ok(v) = std::env::var("HARNESS_LLM_MAX_RETRIES") {
            if let Ok(n) = v.parse() {
                cfg.llm_max_retries = n;
            }
        }
        if let Ok(v) = std::env::var("HARNESS_LLM_RETRY_BACKOFF_MS") {
            if let Ok(n) = v.parse() {
                cfg.llm_retry_backoff_ms = n;
            }
        }
        // MCP server：HARNESS_MCP_CLIENTS="name=kb,url=http://host:9000/mcp;name=...,url=..."
        if let Ok(v) = std::env::var("HARNESS_MCP_CLIENTS") {
            cfg.mcp_clients = parse_mcp_clients(&v);
        }
        if let Ok(v) = std::env::var("HARNESS_STORE_DIR") {
            cfg.store_dir = Some(PathBuf::from(v));
        }
        if let Ok(v) = std::env::var("HARNESS_STORE_REDACT") {
            cfg.store_redact = matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes");
        }
        if let Ok(v) = std::env::var("HARNESS_STORE_LIST_LIMIT") {
            if let Ok(n) = v.parse() {
                cfg.store_list_limit = n;
            }
        }

        if let Some(l) = &cli.listen {
            cfg.listen = l.clone();
        }
        if let Some(r) = &cli.resources {
            cfg.resources_dir = r.clone();
        }

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
