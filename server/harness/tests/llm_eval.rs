//! LLM 评测集（T4.4）：用 `cases.jsonl` 对 `/chat` 链路的辨证质量自动评分
//!
//! 与 `cases.rs` 的分工：
//! - `cases.rs` 是**确定性**基准（关键词 / 证候 / 方剂数据自洽性），不需要 LLM；
//! - 本文件是**LLM 质量**评测：真的把病例跑一遍辨证，看模型能不能说中期望证候。
//!
//! 因为要连真实 LLM（耗时、花钱、结果有随机性），本评测**默认跳过**：
//! 只有 `HARNESS_EVAL=1` 时才真正运行，适合 nightly 或本地人工跑分，
//! 不作为 PR 门禁（见 plan.md 风险条目 3）。
//!
//! ```bash
//! # 容器内，连宿主机 LM Studio
//! HARNESS_EVAL=1 HARNESS_LLM_BASE_URL=http://host.docker.internal:11223/v1 \
//!   cargo test -p harness --test llm_eval -- --nocapture
//! ```
//!
//! 可调环境变量：
//! - `HARNESS_EVAL=1`                 启用评测（缺省跳过）
//! - `HARNESS_EVAL_LIMIT=20`          最多评多少条（默认 20）
//! - `HARNESS_EVAL_TIMEOUT_SECS=120`  单条超时（超时计 0 分，不挂住整轮）
//! - `HARNESS_EVAL_MIN_SCORE=0`       总分低于该值则测试失败（默认 0，只出报告）

use harness::agents::Registry;
use harness::config::HarnessConfig;
use harness::model::{Capability, Message};
use harness::orchestrator;
use harness::resources::ResourceBundle;
use harness::skills::build_default_registry;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// 读取病例；只保留带期望证候的（无基准无从打分）
fn load_eval_cases() -> Vec<serde_json::Value> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("cases.jsonl");
    let text = std::fs::read_to_string(&p).expect("cases.jsonl 缺失");
    let mut seen: HashSet<String> = HashSet::new();
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|c| {
            c.get("syndromes")
                .and_then(|v| v.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false)
        })
        // cases.jsonl 里有大量同语料的重复病例：按语料去重，避免评分被重复样本带偏
        .filter(|c| seen.insert(case_corpus(c)))
        .collect()
}

fn case_corpus(case: &serde_json::Value) -> String {
    let mut parts = Vec::new();
    if let Some(c) = case.get("complaint").and_then(|v| v.as_str()) {
        parts.push(c.to_string());
    }
    if let Some(evos) = case.get("evidences").and_then(|v| v.as_array()) {
        for e in evos {
            if let Some(v) = e.get("value").and_then(|v| v.as_str()) {
                parts.push(v.to_string());
            }
        }
    }
    parts.join("\n")
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// 读环境变量，**空串视为未设置**：CI 里 `HARNESS_MODEL: ""` 是常见写法，
/// 若直接拿来覆盖会把模型名清空（默认模型反而丢失）。
fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// 单条病例的评分结果
#[derive(serde::Serialize)]
struct CaseScore {
    id: String,
    expected: Vec<String>,
    hits: Vec<String>,
    misses: Vec<String>,
    score: f64,
    error: Option<String>,
}

#[tokio::test]
async fn differentiation_quality_eval() {
    if std::env::var("HARNESS_EVAL").as_deref() != Ok("1") {
        println!("跳过 LLM 评测：设置 HARNESS_EVAL=1 启用（需要真实 LLM 端点）");
        return;
    }

    // 缺省走 `Default`，未设置（含空串）的环境变量不参与覆盖
    let mut cfg = HarnessConfig {
        resources_dir: PathBuf::from("resources"),
        ..HarnessConfig::default()
    };
    if let Some(v) = env_non_empty("HARNESS_LLM_BASE_URL") {
        cfg.llm_base_url = v;
    }
    if let Some(v) = env_non_empty("HARNESS_MODEL") {
        cfg.model = v;
    }

    let res = harness::resources::load(&cfg.resources_dir).expect("资源加载失败");
    let registry = Registry::new();
    let llm = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(cfg.llm_timeout_secs))
        .build()
        .expect("HTTP 客户端构建失败");
    let skills = build_default_registry(
        &cfg,
        &res,
        llm.clone(),
        std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    );

    let cases = load_eval_cases();
    let limit = env_usize("HARNESS_EVAL_LIMIT", 20);
    let per_case_secs = env_usize("HARNESS_EVAL_TIMEOUT_SECS", 120);
    let min_score = env_f64("HARNESS_EVAL_MIN_SCORE", 0.0);

    let mut scores: Vec<CaseScore> = Vec::new();
    for case in cases.iter().take(limit) {
        let id = case
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("-")
            .to_string();
        let expected: Vec<String> = case
            .get("syndromes")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let messages = vec![Message {
            role: "user".to_string(),
            content: case_corpus(case),
        }];

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(per_case_secs as u64),
            run_differentiation(&registry, &cfg, &res, &llm, &skills, &messages),
        )
        .await;

        let (hits, misses, error) = match outcome {
            Ok(Ok(text)) => {
                let mut hits = Vec::new();
                let mut misses = Vec::new();
                for e in &expected {
                    if text.contains(e.as_str()) {
                        hits.push(e.clone());
                    } else {
                        misses.push(e.clone());
                    }
                }
                (hits, misses, None)
            }
            Ok(Err(e)) => (vec![], expected.clone(), Some(e.to_string())),
            Err(_) => (
                vec![],
                expected.clone(),
                Some(format!("单条超时（{per_case_secs}s）")),
            ),
        };

        let score = if expected.is_empty() {
            0.0
        } else {
            hits.len() as f64 / expected.len() as f64
        };
        scores.push(CaseScore {
            id,
            expected,
            hits,
            misses,
            score,
            error,
        });
    }

    let total = scores.len();
    let overall = if total == 0 {
        0.0
    } else {
        scores.iter().map(|s| s.score).sum::<f64>() / total as f64
    };
    let perfect = scores.iter().filter(|s| s.score >= 1.0).count();

    let report = serde_json::json!({
        "model": cfg.model,
        "llm_base_url": cfg.llm_base_url,
        "cases": total,
        "overall_score": (overall * 1000.0).round() / 1000.0,
        "full_match_rate": if total == 0 { 0.0 } else { (perfect as f64 / total as f64 * 1000.0).round() / 1000.0 },
        "min_score": min_score,
        "details": scores,
    });

    let out_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(out_dir).ok();
    let out = out_dir.join("llm_eval_report.json");
    std::fs::write(&out, serde_json::to_string_pretty(&report).unwrap()).ok();

    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    println!("评测报告已写入：{}", out.display());

    assert!(
        overall >= min_score,
        "辨证质量分 {overall:.3} 低于门槛 {min_score}（共 {total} 条）"
    );
}

/// 跑一次辨证步骤，返回正文
async fn run_differentiation(
    registry: &Registry,
    cfg: &HarnessConfig,
    res: &ResourceBundle,
    llm: &reqwest::Client,
    skills: &harness::skills::SkillRegistry,
    messages: &[Message],
) -> anyhow::Result<String> {
    let (_, text, _, _) = orchestrator::run_single(
        registry,
        cfg,
        res,
        llm,
        skills,
        Capability::Differentiation,
        messages,
        &serde_json::json!({}),
    )
    .await?;
    Ok(text)
}
