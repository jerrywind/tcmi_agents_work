//! 案例回归测试：用 backend 的 `cases.jsonl` 作为基准，验证 harness 的
//! 确定性逻辑（关键词证据匹配 / 证候推断 / 方剂调护检索）能从历史案例的
//! 证据正确还原期望证候与治法。
//!
//! 不依赖 LLM：只验证「资源数据 + 纯函数」链路是否自洽，作为 YAML 资源
//! 完整性的回归护栏。LLM 端到端评测见 `tests/e2e_llm.rs`（需配置 LLM 端点）。
//!
//! 运行：`cargo test -p harness --test cases`

use harness::agents::{infer_syndrome_slug, match_keywords};
use harness::knowledge::{find_care, find_formula};
use harness::model::Message;
use harness::resources::{load, ResourceBundle};
use std::path::Path;

fn load_cases() -> Vec<serde_json::Value> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("cases.jsonl");
    let text = std::fs::read_to_string(&p)
        .expect("cases.jsonl 缺失：请从 backend/cases 复制到此");
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("cases.jsonl 行解析失败"))
        .collect()
}

fn syndrome_slug_by_name(res: &ResourceBundle, name: &str) -> Option<String> {
    res.syndromes
        .iter()
        .find(|s| s.name == name || s.slug == name)
        .map(|s| s.slug.clone())
}

fn case_corpus(case: &serde_json::Value) -> String {
    // 收集主诉 + 所有证据 value，作为匹配语料
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

#[test]
fn cases_load_and_resources_consistent() {
    let res = load(Path::new("resources")).expect("资源加载失败");
    let cases = load_cases();
    assert!(!cases.is_empty(), "cases.jsonl 不应为空");

    let mut checked = 0usize;
    let mut missing_syndrome = Vec::new();
    for case in &cases {
        let id = case.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        let syndromes = case
            .get("syndromes")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        // 无明确期望证候基准的 case 跳过证候相关断言
        if syndromes.is_empty() {
            continue;
        }

        let corpus = case_corpus(case);
        let msgs = vec![Message {
            role: "user".to_string(),
            content: corpus.clone(),
        }];
        let has_evidence = case
            .get("evidences")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false);

        // 1) 关键词证据匹配：有证据的 case，语料应命中至少一个证据标签
        if has_evidence {
            let hits = match_keywords(&res, &corpus);
            assert!(
                !hits.is_empty(),
                "case {id} 的语料未命中任何关键词证据：{corpus}"
            );
        }

        // 2) 期望证候必须在资源中存在（资源完整性护栏，始终校验）
        for s in &syndromes {
            let name = s.as_str().unwrap_or("");
            if syndrome_slug_by_name(&res, name).is_none() {
                missing_syndrome.push((id.to_string(), name.to_string()));
            }
        }

        // 3) 纯函数推断：有证据且资源含期望证候时，推断候选集应覆盖全部期望证候
        if has_evidence {
            let expected_slugs: Vec<String> = syndromes
                .iter()
                .filter_map(|v| v.as_str())
                .filter_map(|name| syndrome_slug_by_name(&res, name))
                .collect();
            if !expected_slugs.is_empty() {
                let inferred: Vec<String> = infer_syndrome_slug(&res, &msgs);
                for exp in &expected_slugs {
                    assert!(
                        inferred.contains(exp),
                        "case {id} 推断候选集 {inferred:?} 未覆盖期望证候 slug「{exp}」（期望集 {expected_slugs:?}）"
                    );
                }
            }
        }

        checked += 1;
    }

    assert!(checked > 0);
    // 资源完整性：报告缺失的证候（驱动 E 步扩充 YAML）
    if !missing_syndrome.is_empty() {
        let uniq: std::collections::BTreeSet<_> =
            missing_syndrome.iter().map(|(_, n)| n.clone()).collect();
        panic!(
            "以下 {} 个期望证候在 resources/syndromes.yaml 中缺失，需补充：{:?}",
            uniq.len(),
            uniq
        );
    }
}

#[test]
fn expected_syndromes_have_formulas_and_care() {
    let res = load(Path::new("resources")).expect("资源加载失败");
    let cases = load_cases();
    for case in &cases {
        let syndromes = case
            .get("syndromes")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for s in &syndromes {
            let name = s.as_str().unwrap_or("");
            if let Some(slug) = syndrome_slug_by_name(&res, name) {
                let formulas = find_formula(&res, &slug);
                let care = find_care(&res, &slug);
                assert!(
                    !formulas.is_empty() || !care.is_empty(),
                    "证候「{name}」既无方剂也无调护数据"
                );
            }
        }
    }
}
