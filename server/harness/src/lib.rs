//! harness crate：风蓝 TCM 后端（Rust 复刻 backend）
//!
//! 架构：
//! - `config`   运行配置
//! - `model`    协议/数据模型
//! - `resources` 可改 YAML 资源（证候/问诊/安全/方剂/调护/提示词/路由）
//! - `agents`   七个 sub-agent（望闻问切/辨证/安全/治疗）+ 注册表
//! - `orchestrator` 诊断 Loop 流程引擎
//! - `knowledge`  PPG 解析 / 用药安全 / 方剂检索
//! - `rag_health` RAG 服务可达性探测（T7.5，`/health` 暴露）
//! - `skills`    工具调用（MCP / HTTP）
//! - `mcp`       MCP client/server
//! - `trace`     调用级埋点（耗时 / token / 工具 / 错误）
//! - `http`      axum HTTP 服务

pub mod agents;
pub mod config;
pub mod http;
pub mod knowledge;
pub mod mcp;
pub mod model;
pub mod orchestrator;
pub mod rag_health;
pub mod resources;
pub mod skills;
pub mod store;
pub mod trace;

use anyhow::Context;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::HarnessConfig;
use crate::resources::ResourceBundle;
use crate::store::ReportStore;

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
    /// 当前科室（临床学科标签）：辨证后由编排器写入，`tcm-rag` 读取用于
    /// 动态收窄检索范围。技能执行体是闭包、拿不到 agent 上下文，故走共享状态。
    pub departments: skills::SharedDepartments,
    /// 报告存储（T5.1；未配置 store_dir 时为空实现）
    pub store: Arc<ReportStore>,
    /// RAG 服务可达性（T7.5）：后台任务定期探测，`/health` 读这份缓存。
    /// 用 std 的 RwLock：读的是快照、不跨 await，异步锁没有意义。
    pub rag: rag_health::SharedRagStatus,
}

impl AppState {
    /// RAG 状态快照（锁中毒时仍返回内容，诊断信息不该因中毒而拿不到）
    pub fn rag_status(&self) -> rag_health::RagStatus {
        match self.rag.read() {
            Ok(g) => g.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
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

        // 技能注册表：内置工具（9 个 + treatment 专属 2 个），再按配置挂载外部 MCP
        // 显式用 std 的 RwLock：技能执行体在同步闭包里读它，不该引入异步锁
        let departments: skills::SharedDepartments = Arc::new(std::sync::RwLock::new(Vec::new()));
        let mut skills =
            skills::build_default_registry(&config, &resources, llm.clone(), departments.clone());
        if !config.mcp_clients.is_empty() {
            skills::mount_mcp_clients(&mut skills, &config, &llm).await;
        }

        // 报告存储：未配置 store_dir 时为空实现，行为与持久化前一致
        let store = ReportStore::new(config.store_dir.clone(), config.store_redact)
            .context("初始化报告存储失败")?;
        if store.is_enabled() {
            tracing::info!(
                dir = %store.dir().map(|d| d.display().to_string()).unwrap_or_default(),
                redact = config.store_redact,
                "报告持久化已启用"
            );
        }

        // RAG 可达性探测（T7.5）：先 Arc 化配置，探测任务与 AppState 共用同一份
        let cfg = Arc::new(config);
        let rag: rag_health::SharedRagStatus =
            Arc::new(std::sync::RwLock::new(rag_health::RagStatus::default()));
        rag_health::spawn_probe_task(cfg.clone(), rag.clone());

        Ok(Self {
            config: cfg,
            resources: Arc::new(RwLock::new(resources)),
            llm,
            registry,
            skills: Arc::new(skills),
            departments,
            store: Arc::new(store),
            rag,
        })
    }

    /// 热重载资源（hot_reload=true 时由接口触发）
    pub async fn reload_resources(&self) -> anyhow::Result<()> {
        let bundle = resources::load(&self.config.resources_dir)?;
        *self.resources.write().await = bundle;
        Ok(())
    }
}
