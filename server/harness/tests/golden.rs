//! 黄金病例集：**辨证准确性**回归基线（H7）
//!
//! ## 为什么需要它
//!
//! `tests/cases.rs` 用的是 `cases.jsonl`——93 条合成数据，只有 5 种主诉
//! （37 条主诉就是字面量 `x`）、3 种证候组合，且**标签自相矛盾**
//! （主诉「口苦、口臭、大便粘马桶、身体困重」被标成「风寒感冒 + 肝郁气滞」）。
//! 它的断言是「期望证候出现在候选集里」，而候选集 = 所有得分 > 0 的证候——
//! **全库命中都能通过**。所以它是资源完整性护栏，**衡量不了准不准**。
//!
//! 本文件补的就是这一块：
//! - `positive` 用例断言**首位命中**（不是「在候选集里」）；
//! - `out_of_library` 用例断言**不产出主证**（库外证候不许被硬塞进库内某证）。
//!
//! 第二条尤其关键：它守的是 H3 那个「知道自己不知道」的出口。
//! 没有它，任何一次打分公式调整都可能悄悄把降级路径改回「必选其一」，
//! 而所有既有测试照样全绿。
//!
//! ## 数据从哪来
//!
//! `golden_cases.jsonl` 是**人工构造**的教科书式病例，不是真实病例，
//! 也不含专家标注。它能守住「规则层按设计工作」，
//! 守不住「证候库的症状写错了」——后者需要执业中医师复核
//! `resources/syndromes.yaml`（见 `docs/tasks.md` 阶段 H 备注）。
//!
//! 不依赖 LLM，纯函数断言，可进 PR 门禁。
//!
//! 运行：`cargo test -p harness --test golden`

use harness::agents::differentiation::assess;
use harness::model::Message;
use harness::resources::{load, ResourceBundle};
use std::path::Path;

fn res() -> ResourceBundle {
    load(Path::new("resources")).expect("资源加载失败")
}

fn load_cases() -> Vec<serde_json::Value> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("golden_cases.jsonl");
    let text = std::fs::read_to_string(&p).expect("golden_cases.jsonl 缺失：位于 harness 包根目录");
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).expect("golden_cases.jsonl 行解析失败")
        })
        .collect()
}

fn msgs(corpus: &str) -> Vec<Message> {
    vec![Message {
        role: "user".to_string(),
        content: corpus.to_string(),
    }]
}

/// 正例：主证必须**就是**期望证候（首位命中，而非出现在候选集里）
#[test]
fn positive_cases_hit_expected_syndrome_first() {
    let r = res();
    let cases = load_cases();
    let positives: Vec<_> = cases
        .iter()
        .filter(|c| c.get("kind").and_then(|v| v.as_str()) == Some("positive"))
        .collect();
    assert!(!positives.is_empty(), "golden_cases.jsonl 应含正例");

    let mut failed = Vec::new();
    for c in &positives {
        let id = c.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        let expect = c.get("expect").and_then(|v| v.as_str()).unwrap_or("");
        let corpus = c.get("corpus").and_then(|v| v.as_str()).unwrap_or("");
        let d = assess(&r, &msgs(corpus));
        match &d.primary {
            Some(p) if p.slug == expect => {}
            Some(p) => failed.push(format!(
                "{id}：期望「{expect}」，实得「{}」（置信度 {:.2}，\n\
                 \x20   全部候选 {}\n\x20   语料：{corpus}）",
                p.slug,
                p.confidence,
                d.ranked
                    .iter()
                    .map(|x| format!("{}:{:.2}", x.slug, x.score))
                    .take(4)
                    .collect::<Vec<_>>()
                    .join(" / "),
            )),
            None => failed.push(format!(
                "{id}：期望「{expect}」，实得「未匹配到证候」\n\
                 \x20   near: {}\n\x20   语料：{corpus}",
                d.near
                    .iter()
                    .map(|x| format!(
                        "{}:{:.2}（缺主症 {}）",
                        x.slug,
                        x.score,
                        x.missing_key_symptoms.join("、")
                    ))
                    .collect::<Vec<_>>()
                    .join(" / ")
            )),
        }
    }

    assert!(
        failed.is_empty(),
        "{} 条正例未首位命中：\n{}",
        failed.len(),
        failed.join("\n")
    );
}

/// 负例：库外证候**不得**产出主证——必须走「不知道」这条出口
#[test]
fn out_of_library_cases_yield_no_primary() {
    let r = res();
    let cases = load_cases();
    let negatives: Vec<_> = cases
        .iter()
        .filter(|c| c.get("kind").and_then(|v| v.as_str()) == Some("out_of_library"))
        .collect();
    assert!(!negatives.is_empty(), "golden_cases.jsonl 应含负例");

    let mut failed = Vec::new();
    for c in &negatives {
        let id = c.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        let corpus = c.get("corpus").and_then(|v| v.as_str()).unwrap_or("");
        let d = assess(&r, &msgs(corpus));
        if let Some(p) = &d.primary {
            failed.push(format!(
                "{id}（{}）：库外语料却被判为「{}」（置信度 {:.2}）\n\
                 \x20   语料：{corpus}",
                c.get("note").and_then(|v| v.as_str()).unwrap_or(""),
                p.name,
                p.confidence
            ));
        }
    }

    assert!(
        failed.is_empty(),
        "{} 条库外负例被判成了库内证候（H3 的降级出口失效）：\n{}",
        failed.len(),
        failed.join("\n")
    );
}

/// 未匹配时必须留下「最接近谁、缺哪条主症」，供模型与前端说清差在哪
#[test]
fn unmatched_result_carries_near_miss_reason() {
    let r = res();
    let cases = load_cases();
    // 只挑「有命中但未达门槛」的负例：n05 只中次症，near 必不为空
    let target = cases
        .iter()
        .find(|c| c.get("id").and_then(|v| v.as_str()) == Some("n05"))
        .expect("n05 用例缺失");
    let corpus = target.get("corpus").and_then(|v| v.as_str()).unwrap();
    let d = assess(&r, &msgs(corpus));

    assert!(!d.matched);
    assert!(!d.near.is_empty(), "n05 有次症命中，near 不应为空");
    assert!(
        d.near.iter().all(|n| !n.missing_key_symptoms.is_empty()),
        "near 里的候选必须给出缺哪些主症，否则无法说明为什么没匹配：{:?}",
        d.near
    );
    // 给模型的提示里要同时出现「未匹配」与「缺主症」
    let brief = d.brief();
    assert!(brief.contains("未匹配到明确证候"), "brief：{brief}");
    assert!(brief.contains("缺主症"), "brief：{brief}");
    assert!(
        brief.contains("无法定证"),
        "brief 必须授权模型说不知道：{brief}"
    );
}

/// 正例语料必须真的能定证——若某条被打分改动压到阈值以下，
/// 这里会先于 `positive_cases_hit_expected_syndrome_first` 给出更明确的信号。
///
/// （H1/H2 的打分公式断言在 `tests/behavior.rs`，本文件只守病例集。）
#[test]
fn golden_cases_are_well_formed() {
    let cases = load_cases();
    assert!(cases.len() >= 20, "病例集不应少于 20 条：{}", cases.len());
    for c in &cases {
        let id = c.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        let kind = c.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            matches!(kind, "positive" | "out_of_library"),
            "{id} 的 kind 非法：{kind}"
        );
        assert!(
            c.get("corpus")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.trim().is_empty()),
            "{id} 缺 corpus"
        );
        assert!(
            c.get("note").and_then(|v| v.as_str()).is_some(),
            "{id} 缺 note"
        );
        if kind == "positive" {
            assert!(
                c.get("expect").and_then(|v| v.as_str()).is_some(),
                "{id} 缺 expect"
            );
        }
    }
}
