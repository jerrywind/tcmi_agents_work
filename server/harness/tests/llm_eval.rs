//! LLM 评测集（T4.4）：用黄金病例集评辨证质量
//!
//! 与 `golden.rs` 的分工：
//! - `golden.rs` 是**确定性**回归：直接调 `assess()`，不需要 LLM，进 CI；
//! - 本文件是**LLM 质量**评测：真的把每条病例跑一遍辨证 agent，
//!   看**模型正文**说没说对——规则层算对了，模型也可能在正文里讲成另一回事。
//!
//! 因为要连真实 LLM（耗时、结果有随机性），本评测**默认跳过**：
//! 只有 `HARNESS_EVAL=1` 时才真正运行，适合 nightly 或本地人工跑分，
//! 不作为 PR 门禁（见 plan.md 风险条目 3）。
//!
//! ```bash
//! # 容器内，连宿主机 LM Studio
//! HARNESS_EVAL=1 HARNESS_LLM_BASE_URL=http://host.docker.internal:11223/v1 \
//!   cargo test -p harness --test llm_eval -- --nocapture
//! ```
//!
//! # 为什么改用 `golden_cases.jsonl`
//!
//! 此前用 `cases.jsonl`，而那份数据是合成的——93 条只有 5 种主诉，
//! 且**标签自相矛盾**（脾胃湿热的主诉被标成「风寒感冒 + 肝郁气滞」）。
//! 评分方式又是 `正文.contains(期望证候)`，期望值写的是 slug 时永远匹配不上中文正文。
//! 用这样的数据跑出来的分数**既不能当质量指标，也不能当回归信号**。
//! `golden_cases.jsonl` 的标签经过人工核对，且区分正例与库外负例，
//! 可以双向评分：**该定证的定对了吗？该说不知道的说了吗？**
//!
//! # 关于默认条数
//!
//! `HARNESS_EVAL_LIMIT` 默认 50 而非 20：黄金病例集有 21 条，
//! 默认 20 会把末尾的库外负例整段截掉——而「该说不知道时说了没有」
//! 恰是本集的重点，被静默跳过的代价太大。
//!
//! 可调环境变量：
//! - `HARNESS_EVAL=1`                 启用评测（缺省跳过）
//! - `HARNESS_EVAL_LIMIT=50`          最多评多少条（默认 50）
//! - `HARNESS_EVAL_TIMEOUT_SECS=120`  单条超时（超时计 0 分，不挂住整轮）
//! - `HARNESS_EVAL_MIN_SCORE=0`       总分低于该值则测试失败（默认 0，只出报告）

use harness::agents::Registry;
use harness::config::HarnessConfig;
use harness::model::{Capability, Message};
use harness::orchestrator;
use harness::resources::ResourceBundle;
use harness::skills::build_default_registry;
use std::path::{Path, PathBuf};

/// 读取黄金病例
fn load_eval_cases() -> Vec<serde_json::Value> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("golden_cases.jsonl");
    let text = std::fs::read_to_string(&p).expect("golden_cases.jsonl 缺失");
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .collect()
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
    kind: String,
    expected: String,
    /// 规则层（`assess()`）实际判出的主证 slug；库外负例期望为 `null`
    rule_primary: Option<String>,
    /// 规则层是否符合预期
    rule_hit: bool,
    /// 模型正文是否符合预期
    llm_hit: bool,
    error: Option<String>,
}

/// 模型有没有在正文里承认「定不了证」
///
/// 库外负例的关键不是模型辨出什么，而是**它有没有把猜测包装成结论**——
/// 那正是 H3 要杜绝的。措辞不统一，按关键词宽松匹配。
fn llm_admits_unknown(text: &str) -> bool {
    ["未匹配", "未辨出", "无法定证", "不明确", "证据不足"]
        .iter()
        .any(|k| text.contains(k))
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
    // 必须显式读：`Default` 不读环境变量（那是 `from_env()` 的事），
    // 少这一行本评测会对端点直接 401，21 条全 0 分却「测试通过」——
    // 门槛默认又是 0，于是连失败都看不出来。
    if let Some(v) = env_non_empty("HARNESS_LLM_API_KEY") {
        cfg.llm_api_key = v;
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
    let limit = env_usize("HARNESS_EVAL_LIMIT", 50);
    let per_case_secs = env_usize("HARNESS_EVAL_TIMEOUT_SECS", 120);
    let min_score = env_f64("HARNESS_EVAL_MIN_SCORE", 0.0);

    let mut scores: Vec<CaseScore> = Vec::new();
    for case in cases.iter().take(limit) {
        let id = case
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("-")
            .to_string();
        let kind = case
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("positive")
            .to_string();
        let expected = case
            .get("expect")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let corpus = case
            .get("corpus")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let messages = vec![Message {
            role: "user".to_string(),
            content: corpus,
        }];

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(per_case_secs as u64),
            run_differentiation(&registry, &cfg, &res, &llm, &skills, &messages),
        )
        .await;

        let (rule_hit, llm_hit, rule_primary, error) = match outcome {
            Ok(Ok((text, structured))) => {
                let primary = structured
                    .as_ref()
                    .and_then(|s| s.get("primary"))
                    .and_then(|p| p.as_object()) // primary 为 null 时这里返回 None
                    .and_then(|p| p.get("slug"))
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());

                if kind == "out_of_library" {
                    // 该说不知道的：规则层不许硬塞一个库内证，
                    // 正文也不许把它讲成确定结论
                    (primary.is_none(), llm_admits_unknown(&text), primary, None)
                } else {
                    // 该定证的：规则层首位命中，且正文说出了这个证名
                    let name = res
                        .syndrome(&expected)
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| expected.clone());
                    (
                        primary.as_deref() == Some(expected.as_str()),
                        text.contains(&name),
                        primary,
                        None,
                    )
                }
            }
            Ok(Err(e)) => (false, false, None, Some(e.to_string())),
            Err(_) => (
                false,
                false,
                None,
                Some(format!("单条超时（{per_case_secs}s）")),
            ),
        };

        scores.push(CaseScore {
            id,
            kind,
            expected,
            rule_primary,
            rule_hit,
            llm_hit,
            error,
        });
    }

    let total = scores.len();
    let avg = |f: fn(&CaseScore) -> bool| {
        if total == 0 {
            0.0
        } else {
            scores.iter().filter(|s| f(s)).count() as f64 / total as f64
        }
    };
    let rule_score = avg(|s| s.rule_hit);
    let llm_score = avg(|s| s.llm_hit);
    let overall = (rule_score + llm_score) / 2.0;
    let round3 = |x: f64| (x * 1000.0).round() / 1000.0;

    let report = serde_json::json!({
        "model": cfg.model,
        "llm_base_url": cfg.llm_base_url,
        "cases": total,
        // 规则层是确定性的：它错了说明打分公式或证候库有问题，与模型无关
        "rule_score": round3(rule_score),
        // 正文层反映提示词与模型遵循度
        "llm_score": round3(llm_score),
        "overall_score": round3(overall),
        "min_score": min_score,
        "details": scores,
    });

    let out_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(out_dir).ok();
    let out = out_dir.join("llm_eval_report.json");
    std::fs::write(&out, serde_json::to_string_pretty(&report).unwrap()).ok();

    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    println!("评测报告已写入：{}", out.display());

    // 先把「评测根本没跑起来」和「跑起来了但质量差」分开。
    //
    // 端点不通或 API Key 不对时，每一条都会落到 error 分支，分数是 0；
    // 而 `HARNESS_EVAL_MIN_SCORE` 默认为 0，于是整轮**测试通过**。
    // 会静默通过的检查等于没有检查——这里必须显式炸出来。
    let failed = scores.iter().filter(|s| s.error.is_some()).count();
    if total > 0 && failed == total {
        panic!(
            "全部 {total} 条都执行失败，评测没有真正跑起来\
             （多半是 LLM 端点或 API Key 不对，而非辨证质量差）。\n首条错误：{:?}",
            scores[0].error
        );
    }
    if total > 0 && failed > 0 {
        println!("注意：{failed}/{total} 条执行失败（未计入质量分），详见报告 details");
    }

    assert!(
        overall >= min_score,
        "辨证质量分 {overall:.3}（规则层 {rule_score:.3} / 正文 {llm_score:.3}）\
         低于门槛 {min_score}（共 {total} 条，其中 {failed} 条执行失败）"
    );
}

/// 跑一次辨证步骤，返回（正文，结构化结果）
async fn run_differentiation(
    registry: &Registry,
    cfg: &HarnessConfig,
    res: &ResourceBundle,
    llm: &reqwest::Client,
    skills: &harness::skills::SkillRegistry,
    messages: &[Message],
) -> anyhow::Result<(String, Option<serde_json::Value>)> {
    let (_, text, _, structured) = orchestrator::run_single(
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
    Ok((text, structured))
}
