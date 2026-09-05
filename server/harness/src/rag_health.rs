//! RAG 服务可达性探测（T7.5）
//!
//! ## 为什么要单独做这件事
//!
//! `config.yaml` 的 `rag_endpoint` 默认指向 `http://llm_server:8000/rag/retrieve/text`——
//! 那是 compose 网络里的**服务名**，单机 `docker run` 时根本解析不到。
//! 此时 `tcm-rag` 技能按设计「优雅降级」，返回一条提示串；模型拿到后照样往下写，
//! **开方就悄悄失去了 694 部典籍的依据**，而调用方从响应里完全看不出来。
//!
//! 「没检索到典籍」和「典籍里没有」是两回事：后者是结论，前者是故障。
//! 静默降级让两者在输出里长得一模一样，这正是可观测性最该堵的口子。
//!
//! ## 做法
//!
//! 后台任务定期探测，把状态缓存在共享状态里，`GET /health` 直接读缓存——
//! 探测是网络调用，不能挂在每次健康检查上（那会让 `/health` 变得又慢又不确定）。

use crate::config::HarnessConfig;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// 探测间隔：RAG 服务挂掉到被发现，最坏也就等这么久
const PROBE_INTERVAL: Duration = Duration::from_secs(60);

/// 单次探测超时（实测查询 41ms，5s 绰绰有余；超时即判不可用，不硬等）
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// RAG 服务状态（`/health` 暴露）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RagStatus {
    /// 是否配置了端点。未配置时 `reachable` 恒为 `None`
    pub configured: bool,
    /// 最近一次探测是否成功。`None` = 还没探测过或未配置
    pub reachable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// 距上次探测成功的秒数；从未成功过则为 `None`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_last_ok_secs: Option<u64>,
}

impl RagStatus {
    /// 未配置端点时的状态：不是故障，是没启用
    pub fn unconfigured() -> Self {
        Self {
            configured: false,
            ..Default::default()
        }
    }
}

pub type SharedRagStatus = Arc<RwLock<RagStatus>>;

/// 启动后台探测任务
///
/// 未配置端点时不启任务，只在启动时告警一次——
/// 让「典籍检索压根没开」也能在日志里被看见。
pub fn spawn_probe_task(cfg: Arc<HarnessConfig>, status: SharedRagStatus) {
    let endpoint = cfg
        .rag_endpoint
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let Some(endpoint) = endpoint else {
        write(&status, RagStatus::unconfigured());
        tracing::warn!(
            "未配置 rag_endpoint：tcm-rag 不可用，开方将缺少典籍依据\
             （设 HARNESS_RAG_ENDPOINT 启用；compose 网络外需换成宿主机可达地址）"
        );
        return;
    };

    tokio::spawn(async move {
        let mut last_ok_at: Option<Instant> = None;
        loop {
            let outcome = probe(&endpoint).await;
            let since = last_ok_at.map(|t| t.elapsed().as_secs());

            let next = match &outcome {
                Ok(()) => {
                    if last_ok_at.is_none() {
                        tracing::info!(endpoint = %endpoint, "RAG 服务已连通，典籍检索可用");
                    }
                    last_ok_at = Some(Instant::now());
                    RagStatus {
                        configured: true,
                        reachable: Some(true),
                        endpoint: Some(endpoint.clone()),
                        last_error: None,
                        since_last_ok_secs: Some(0),
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        endpoint = %endpoint,
                        error = %e,
                        "RAG 服务不可用：本次问诊的方剂与本草依据将缺少典籍支撑"
                    );
                    RagStatus {
                        configured: true,
                        reachable: Some(false),
                        endpoint: Some(endpoint.clone()),
                        last_error: Some(e.clone()),
                        since_last_ok_secs: since,
                    }
                }
            };
            write(&status, next);

            tokio::time::sleep(PROBE_INTERVAL).await;
        }
    });
}

/// 探活：发一个最小请求，确认服务**真的能检索**而不只是端口开着
async fn probe(endpoint: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .map_err(|e| format!("构造 HTTP 客户端失败：{e}"))?;

    // 与 tcm-rag 的契约一致（POST + query/top_k），避免探测路径与真实调用走两套
    let resp = client
        .post(endpoint)
        .json(&serde_json::json!({"query": "健康检查", "top_k": 1}))
        .send()
        .await
        .map_err(|e| format!("连接失败：{e}"))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    // 服务活着但返回的不是 JSON，对调用方同样等于不可用
    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("响应不是合法 JSON：{e}"))?;
    Ok(())
}

/// 写状态。锁中毒时仍写入——状态只是诊断信息，不该因为中毒就整个不可用
fn write(status: &SharedRagStatus, next: RagStatus) {
    match status.write() {
        Ok(mut g) => *g = next,
        Err(poisoned) => *poisoned.into_inner() = next,
    }
}
