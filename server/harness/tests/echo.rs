//! 「回声污染」回归：助手说过的话不得被当成患者证据
//!
//! ## 为什么单独立一个文件
//!
//! 这不是边角问题，是本系统**最容易出现的一类静默失效**：
//! 上一轮助手的总结里复述了证名与症状，下一轮再把这些文本当成证据算一遍，
//! 于是系统会**自我确认**——第一轮猜了个证，第二轮证据「变多」、置信度上升、
//! 覆盖率上升、收敛判定提前通过、证候被锁定、开方按它开。
//! 全程不报错，报告看起来更「确定」，其实是越问越错。
//!
//! `safety_corpus`（orchestrator）早已只取 user 消息，
//! 但辨证打分、收敛判定的覆盖率、问诊的已采集判定都还在读全部消息。
//! 本文件把这三处钉死。
//!
//! 运行：`cargo test -p harness --test echo`（容器内）

use harness::agents::convergence::{evaluate, LoopConfig};
use harness::agents::differentiation::assess;
use harness::model::Message;
use harness::resources::load;
use harness::resources::model::ResourceBundle;
use std::path::Path;

fn bundle() -> ResourceBundle {
    load(Path::new("resources")).expect("资源加载失败")
}

fn user(text: &str) -> Message {
    Message {
        role: "user".to_string(),
        content: text.to_string(),
    }
}

fn assistant(text: &str) -> Message {
    Message {
        role: "assistant".to_string(),
        content: text.to_string(),
    }
}

/// 一段典型的上一轮助手总结：复述证名 + 一大串症状 + 警示语。
/// 这正是会被误当成患者陈述的东西。
const ECHO: &str = "辨证：风热犯肺证。患者咳嗽、咽痛、痰黄稠、口干、发热，\
舌红苔黄，脉浮数。恶寒轻，汗出，头身酸痛，二便调，饮食可，胸腹无异常，睡眠尚可。\
若出现胸痛、呼吸困难、咯血请及时就医。";

#[test]
fn assess_ignores_assistant_echo() {
    let res = bundle();
    let plain = assess(&res, &[user("咳嗽，痰黄稠")]);
    let with_echo = assess(&res, &[user("咳嗽，痰黄稠"), assistant(ECHO)]);

    // 助手的话不是患者陈述，加进去不该改变**任何**打分。
    // 修好之前这里必然不等：ECHO 里那一串症状会被当成新证据再算一遍。
    assert_eq!(
        plain.primary.as_ref().map(|s| s.score),
        with_echo.primary.as_ref().map(|s| s.score),
        "助手回声改变了主证打分（自我确认）：{:?} vs {:?}",
        plain
            .primary
            .as_ref()
            .map(|s| (s.name.clone(), s.score, s.supporting.clone())),
        with_echo
            .primary
            .as_ref()
            .map(|s| (s.name.clone(), s.score, s.supporting.clone())),
    );
    assert_eq!(
        plain.primary.as_ref().map(|s| s.supporting.clone()),
        with_echo.primary.as_ref().map(|s| s.supporting.clone()),
        "助手回声改变了证据链",
    );
}

/// 否定盲区（与回声污染同源：证据必须反映患者**真实**状态）
///
/// `text.contains(sym)` 是纯子串匹配，「没有汗」里含「有汗」、
/// 「不发热」里含「发热」——患者明确否认的表现会被当成存在，
/// 撑高置信度。真机实测：第二轮补一句「没有汗」，
/// 证据里反而多出「有汗」，置信度从 0.76 升到 0.84。
/// 能定出主证的基础主诉（`有汗` / `口渴` 是风热犯肺证的次症）。
/// 用短主诉会因为「主症必备」判不出主证，断言就变成在测空集。
const BASE: &str = "最近三天咳嗽，咽痛，痰黄稠，口干，轻微发热，舌红苔黄，脉浮数";

fn supports(res: &ResourceBundle, text: &str) -> Vec<String> {
    assess(res, &[user(text)])
        .primary
        .map(|p| p.supporting)
        .unwrap_or_default()
}

#[test]
fn negated_manifestation_is_not_evidence() {
    let res = bundle();
    assert!(
        !supports(&res, BASE).is_empty(),
        "基础主诉应能定证，否则下面测的是空集"
    );

    // 同一份主诉，只差「没有汗」里的一个「没」字
    assert!(supports(&res, &format!("{BASE}，有汗"))
        .iter()
        .any(|s| s == "有汗"));
    let denied = supports(&res, &format!("{BASE}，没有汗"));
    assert!(
        !denied.iter().any(|s| s == "有汗"),
        "「没有汗」不得算成「有汗」：{denied:?}"
    );
}

#[test]
fn negation_and_intensifier_are_distinguished() {
    let res = bundle();

    // 「不口渴」是否定 → 不算证据
    let neg = supports(&res, &format!("{BASE}，不口渴"));
    assert!(
        !neg.iter().any(|s| s == "口渴"),
        "「不口渴」不得算成「口渴」：{neg:?}"
    );

    // 「非常口渴」是加重，不是否定 → 仍算证据。
    // 这条是**防过度修正**的护栏：若哪天把「非」也当否定词，
    // 「非常口渴」会被翻成「不口渴」，方向正好相反。
    let intense = supports(&res, &format!("{BASE}，非常口渴"));
    assert!(
        intense.iter().any(|s| s == "口渴"),
        "「非常口渴」里的「口渴」应算证据：{intense:?}"
    );
}

/// 舌象/脉象**不做**否定处理，并且这是安全的——把它固定下来。
///
/// 症状的否定是子串问题（「没有汗」含「有汗」）；舌脉不是：
/// 中文的否定说法是把否定词**插进词组里**（舌不红、脉不浮数），
/// 插进去就不再是「舌红」「脉浮数」的子串了，根本匹配不上。
/// 所以这里不存在误判，不需要处理——**不为不存在的问题加规则**。
///
/// 这条测试是给后来人看的：若哪天有人想「把否定也扩展到舌脉」，
/// 先看这里，别凭对称感改代码。
#[test]
fn tongue_pulse_negation_is_not_a_substring_problem() {
    let res = bundle();
    let base = "最近三天咳嗽，咽痛，痰黄稠，口干，轻微发热";

    // 用**证候库里的原文**（风热犯肺证：舌尖红，苔薄黄 / 脉浮数），
    // 否则测的是「患者说法与库不一致」而非否定
    let plain = assess(&res, &[user(&format!("{base}，舌尖红，苔薄黄，脉浮数"))]);
    let denied = assess(&res, &[user(&format!("{base}，舌不尖红，脉不浮数"))]);

    let has_sign = |r: &harness::agents::differentiation::DifferentiationResult, p: &str| {
        r.primary
            .as_ref()
            .is_some_and(|x| x.supporting.iter().any(|s| s.starts_with(p)))
    };
    assert!(
        has_sign(&plain, "舌象"),
        "原话应有舌象证据：{:?}",
        plain.primary
    );
    assert!(
        has_sign(&plain, "脉象"),
        "原话应有脉象证据：{:?}",
        plain.primary
    );
    // 否定说法匹配不上 → 不会产生「舌象：…」条目
    assert!(
        !has_sign(&denied, "舌象"),
        "「舌不尖红」不应产出舌象证据：{:?}",
        denied.primary
    );
    assert!(
        !has_sign(&denied, "脉象"),
        "「脉不浮数」不应产出脉象证据：{:?}",
        denied.primary
    );
}

/// 影响面量化：在标准病例集上，回声会「凭空定证」多少条
///
/// 修复前 `assess` 把助手的话当证据，于是「回声单独就能定证」的病例
/// 只要患者说了一点点，就会被凭空补出一个主证。这里把**影响面算出来**
/// 而不是只说「修好了」——数量级决定了这条修复值不值得。
///
/// 计数方式：某条病例自身判不出主证，但**只喂回声**却判得出 →
/// 这就是修复前会被凭空定证的病例。
#[test]
fn echo_blast_radius_on_golden_cases() {
    let res = bundle();
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("golden_cases.jsonl");
    let text = std::fs::read_to_string(&p).expect("golden_cases.jsonl 缺失");
    let cases: Vec<serde_json::Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("行解析失败"))
        .collect();
    assert!(!cases.is_empty(), "病例集不应为空");

    let echo_alone = assess(&res, &[user(ECHO)]);
    let mut fabricated = 0usize; // 修复前会被凭空定证的条数
    let mut changed = 0usize; // 修复后仍被回声改变的条数（必须为 0）

    for c in &cases {
        let corpus = c.get("corpus").and_then(|v| v.as_str()).unwrap_or("");
        if corpus.is_empty() {
            continue;
        }
        let alone = assess(&res, &[user(corpus)]);
        let with_echo = assess(&res, &[user(corpus), assistant(ECHO)]);
        let slug = |r: &harness::agents::differentiation::DifferentiationResult| {
            r.primary.as_ref().map(|x| x.slug.clone())
        };
        if slug(&alone) != slug(&with_echo) {
            changed += 1;
        }
        if alone.primary.is_none() && echo_alone.primary.is_some() {
            fabricated += 1;
        }
    }

    eprintln!(
        "[echo] 标准病例 {} 条：修复前会因回声凭空定证 {} 条；修复后被改变 {} 条",
        cases.len(),
        fabricated,
        changed
    );
    assert_eq!(changed, 0, "修复后仍有病例被助手回声改变主证");
}

#[test]
fn coverage_ignores_assistant_echo() {
    let res = bundle();
    let cfg = LoopConfig::default();
    let plain = evaluate(&res, &[user("咳嗽两天")], &cfg, 1);
    let with_echo = evaluate(&res, &[user("咳嗽两天"), assistant(ECHO)], &cfg, 1);

    // 覆盖率是「患者提供了多少必采信息」。
    // 助手自己写的「二便调、饮食可、睡眠尚可」不该算作患者已回答，
    // 否则覆盖率虚高 → 收敛判定提前通过 → 强制放行一份信息不足的结论。
    assert_eq!(
        plain.coverage, with_echo.coverage,
        "助手回声虚增了覆盖率：{:.3} vs {:.3}",
        plain.coverage, with_echo.coverage,
    );
}
