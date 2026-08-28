//! harness crate：风蓝 TCM 后端（Rust 复刻 backend）
//!
//! 架构：
//! - `config`   运行配置
//! - `model`    协议/数据模型
//! - `resources` 可改 YAML 资源（证候/问诊/安全/方剂/调护/提示词/路由）
//! - `agents`   七个 sub-agent（望闻问切/辨证/安全/治疗）+ 注册表
//! - `orchestrator` 诊断 Loop 流程引擎
//! - `knowledge`  PPG 解析 / 用药安全 / 方剂检索
//! - `skills`    工具调用（MCP / HTTP）
//! - `mcp`       MCP client/server
//! - `http`      axum HTTP 服务

pub mod agents;
pub mod config;
pub mod http;
pub mod knowledge;
pub mod mcp;
pub mod model;
pub mod orchestrator;
pub mod resources;
pub mod skills;

use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::Context;

use crate::config::HarnessConfig;
use crate::resources::ResourceBundle;

/// 进程内共享状态
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<HarnessConfig>,
    /// 资源包（热重载时整体替换）
    pub resources: Arc<RwLock<ResourceBundle>>,
    /// LLM 客户端（复用）
    pub llm: reqwest::Client,
    /// sub-agent 注册表
    pub registry: agents::Registry,
    /// 技能/工具注册表（MCP / HTTP）
    pub skills: Arc<skills::SkillRegistry>,
}

impl AppState {
    pub async fn load(config: HarnessConfig) -> anyhow::Result<Self> {
        let resources = resources::load(&config.resources_dir)
            .context("加载 YAML 资源失败，请检查 resources/ 目录")?;
        tracing::info!(
            "已加载资源：证候 {} 条 / 问诊 {} 条 / 安全 {} 条 / 方剂 {} 条",
            resources.syndromes.len(),
            resources.questions.len(),
            resources.red_flags.len(),
            resources.formulas.len()
        );

        let llm = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.llm_timeout_secs))
            .build()?;

        let registry = agents::Registry::new();

        // 技能注册表：内置 9 个工具，按需挂载外部 MCP
        let skills = skills::build_default_registry(&config, &resources, llm.clone());

        Ok(Self {
            config: Arc::new(config),
            resources: Arc::new(RwLock::new(resources)),
            llm,
            registry,
            skills: Arc::new(skills),
        })
    }

    /// 热重载资源（hot_reload=true 时由接口触发）
    pub async fn reload_resources(&self) -> anyhow::Result<()> {
        let bundle = resources::load(&self.config.resources_dir)?;
        *self.resources.write().await = bundle;
        Ok(())
    }
}
