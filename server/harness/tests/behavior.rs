//! 行为回归：T2.x / T3.x 新增能力的**确定性**部分
//!
//! 需要 LLM 的链路不在此覆盖（harness 无 MockProvider）：
//! 这里只验证纯函数与数据结构契约——红旗中断判定、技能归属、埋点累加、
//! 配置解析，以及 `/chat` 响应的 `blocked` / `skipped` / `trace` 字段。
//!
//! 运行：`cargo test -p harness --test behavior`（容器内）

use harness::agents::differentiation::assess;
use harness::agents::{blocking_red_flag, detect_red_flags, is_blocking};
use harness::config::HarnessConfig;
use harness::model::{Capability, Message};
use harness::orchestrator::{diagnosis_payload, Blocked, Diagnosis};
use harness::resources::load;
use harness::skills::build_default_registry;
use harness::trace::{new_trace, record, snapshot, LlmCallStat, StepTrace};
use harness::AppState;
use serde_json::json;
use std::path::Path;

fn bundle() -> harness::resources::ResourceBundle {
    load(Path::new("resources")).expect("资源加载失败")
}

/// 构造进程内 AppState（不监听端口、不连 MCP client，纯本地资源）
async fn app_state() -> AppState {
    let cfg = HarnessConfig::default();
    AppState::load(cfg).await.expect("AppState 加载失败")
}

// ---------------- T3.3 红旗中断判定 ----------------

#[test]
fn critical_red_flag_is_blocking() {
    let res = bundle();
    let hits = detect_red_flags(&res, "患者胸痛并呼吸困难");
    assert_eq!(hits.len(), 2, "胸痛与呼吸困难应各自命中：{hits:?}");

    let blocking = blocking_red_flag(&res, "胸痛").expect("critical 级胸痛必须触发中断");
    assert_eq!(blocking.slug, "chest_pain");
    assert_eq!(blocking.severity, "critical");
    assert!(is_blocking(blocking));
}

#[test]
fn medium_red_flag_does_not_block() {
    let res = bundle();
    // 妊娠为 medium：要提示，但不应中断后续治疗建议
    let hits = detect_red_flags(&res, "怀孕三个月，能用这副药吗");
    assert!(!hits.is_empty(), "妊娠关键词应被命中");
    assert!(
        blocking_red_flag(&res, "怀孕三个月，能用这副药吗").is_none(),
        "medium 级不应触发中断"
    );
}

#[test]
fn no_red_flag_when_text_is_benign() {
    let res = bundle();
    assert!(detect_red_flags(&res, "近三天轻微咳嗽，痰白").is_empty());
}

// ---------------- T2.3 / T2.5 技能归属 ----------------

#[test]
fn treatment_owns_formula_and_care_skills() {
    let res = bundle();
    let cfg = HarnessConfig::default();
    let reg = build_default_registry(&cfg, &res, reqwest::Client::new());

    let names = |c: Capability| -> Vec<String> {
        reg.for_capability(c)
            .iter()
            .map(|s| s.name.clone())
            .collect()
    };

    let treatment = names(Capability::Treatment);
    assert!(
        treatment.contains(&"tcm-formula".to_string()),
        "treatment 应能调用方剂工具：{treatment:?}"
    );
    assert!(
        treatment.contains(&"tcm-care".to_string()),
        "treatment 应能调用调护工具：{treatment:?}"
    );

    // 专属技能不得被其他 capability 看到；全局技能仍对所有人可见
    let inspection = names(Capability::Inspection);
    assert!(
        !inspection.contains(&"tcm-formula".to_string()),
        "专属技能泄漏到其他 capability：{inspection:?}"
    );
    assert!(
        inspection.contains(&"tcm-kb".to_string()),
        "全局技能应对所有 capability 可见：{inspection:?}"
    );
}

#[test]
fn capability_parses_slug_and_chinese_name() {
    assert_eq!(
        Capability::from_name("treatment"),
        Some(Capability::Treatment)
    );
    assert_eq!(Capability::from_name("治疗"), Some(Capability::Treatment));
    assert_eq!(Capability::from_name("不存在的步骤"), None);
    assert_eq!(Capability::ALL.len(), 7);
}

// ---------------- T2.4 MCP 配置解析 ----------------

#[test]
fn config_parses_tool_rounds_retries_and_mcp_clients() {
    let yaml = r#"
max_tool_rounds: 5
llm_max_retries: 0
mcp_clients:
  - name: kb
    url: "http://127.0.0.1:9000/mcp"
    tools: ["search"]
  - name: emr
    url: "http://127.0.0.1:9100/mcp"
    enabled: false
"#;
    let cfg: HarnessConfig = serde_yaml::from_str(yaml).expect("配置解析失败");
    assert_eq!(cfg.max_tool_rounds, 5);
    assert_eq!(cfg.llm_max_retries, 0);
    assert_eq!(cfg.llm_retry_backoff_ms, 500, "未指定时应取默认值");

    assert_eq!(cfg.mcp_clients.len(), 2);
    assert_eq!(cfg.mcp_clients[0].name, "kb");
    assert_eq!(cfg.mcp_clients[0].tools, vec!["search".to_string()]);
    assert!(cfg.mcp_clients[0].enabled, "未指定 enabled 时默认启用");
    assert!(!cfg.mcp_clients[1].enabled);
}

#[test]
fn config_defaults_keep_tool_loop_enabled() {
    let cfg = HarnessConfig::default();
    assert!(cfg.max_tool_rounds >= 1, "工具循环至少应允许一轮");
    assert!(cfg.mcp_clients.is_empty());
}

// ---------------- T3.1 埋点累加 ----------------

#[test]
fn trace_accumulates_tokens_tools_and_errors() {
    let handle = new_trace();
    record(Some(&handle), |m| {
        m.record_llm(&LlmCallStat {
            duration_ms: 10,
            attempts: 2,
            prompt_tokens: Some(5),
            completion_tokens: Some(7),
            total_tokens: Some(12),
            error: None,
        })
    });
    record(Some(&handle), |m| {
        m.record_llm(&LlmCallStat {
            duration_ms: 4,
            attempts: 1,
            prompt_tokens: Some(3),
            completion_tokens: Some(1),
            total_tokens: Some(4),
            error: None,
        });
        m.record_tool("tcm-kb");
        m.record_tool("tcm-rag");
    });

    let m = snapshot(&handle);
    assert_eq!(m.llm_calls, 2);
    assert_eq!(m.llm_attempts, 3, "重试次数应累加");
    assert_eq!(m.llm_duration_ms, 14);
    // 多轮工具调用下 token 按轮次求和，而不是取最大值
    assert_eq!(m.prompt_tokens, Some(8));
    assert_eq!(m.total_tokens, Some(16));
    assert_eq!(
        m.tool_calls,
        vec!["tcm-kb".to_string(), "tcm-rag".to_string()]
    );

    record(Some(&handle), |m| m.record_error("工具执行失败".into()));
    assert_eq!(
        snapshot(&handle).last_error.as_deref(),
        Some("工具执行失败")
    );
}

// ---------------- /chat 响应契约 ----------------

#[test]
fn diagnosis_payload_exposes_blocked_skipped_and_trace() {
    let d = Diagnosis {
        steps: vec![(Capability::Safety, "【安全门警示】…".to_string())],
        final_text: "summary".to_string(),
        failures: vec![],
        skipped: vec![(
            Capability::Treatment,
            "安全门拦截（胸痛·critical），未执行".to_string(),
        )],
        blocked: Some(Blocked {
            by: Capability::Safety,
            slug: "chest_pain".to_string(),
            label: "胸痛".to_string(),
            severity: "critical".to_string(),
            advice: "立即拨打急救电话".to_string(),
        }),
        trace: vec![StepTrace {
            capability: Capability::Safety,
            name: Capability::Safety.zh(),
            duration_ms: 12,
            model: "test-model".to_string(),
            llm_calls: 1,
            llm_attempts: 1,
            llm_duration_ms: 10,
            prompt_tokens: Some(1),
            completion_tokens: Some(2),
            total_tokens: Some(3),
            tool_calls: vec!["tcm-safety".to_string()],
            error: None,
        }],
        structured: vec![],
    };

    let v = diagnosis_payload(&d);
    assert_eq!(v["blocked"], serde_json::json!(true));
    assert_eq!(v["blocked_by"].as_str(), Some("safety"));
    assert_eq!(v["partial"], serde_json::json!(false));
    assert_eq!(v["skipped"][0]["capability"].as_str(), Some("treatment"));
    assert_eq!(v["trace"][0]["name"].as_str(), Some("安全门"));
    assert_eq!(v["trace"][0]["total_tokens"], serde_json::json!(3));
    assert_eq!(v["trace"][0]["tool_calls"][0].as_str(), Some("tcm-safety"));

    // 拦截原因必须落在响应里：调用方无需解析正文即可展示
    let reason = v["block_reason"].as_str().unwrap_or_default();
    assert!(reason.contains("胸痛"), "拦截原因缺少标签：{reason}");
    assert!(reason.contains("critical"), "拦截原因缺少级别：{reason}");
}

// ---------------- T4.1 结构化辨证 ----------------

fn user(text: &str) -> Vec<Message> {
    vec![Message {
        role: "user".to_string(),
        content: text.to_string(),
    }]
}

#[test]
fn wind_cold_corpus_scores_primary_syndrome_with_evidence() {
    let res = bundle();
    let r = assess(
        &res,
        &user("恶寒重发热轻、无汗、头痛、流清涕、咳嗽、痰白稀，脉浮紧"),
    );

    let primary = r.primary.expect("典型风寒语料应辨出主证");
    assert_eq!(primary.slug, "wind_cold_attack_lung");
    assert!(primary.confidence > 0.0 && primary.confidence <= 1.0);
    assert!(
        primary.supporting.contains(&"恶寒重发热轻".to_string()),
        "支持证据应含命中的症状：{:?}",
        primary.supporting
    );
    assert!(
        primary.supporting.contains(&"风寒证据".to_string()),
        "关键词证据标签应计入支持证据：{:?}",
        primary.supporting
    );
    assert!(primary.conflicting.is_empty(), "无相反表现时不应有矛盾证据");
    assert!(primary.pathogenesis.is_some(), "主证应带病机");
}

#[test]
fn contradictory_evidence_is_recorded_and_lowers_confidence() {
    let res = bundle();
    // 刻意只用两条症状：证据量未到满分时，扣减才能反映到置信度上
    let clean = assess(&res, &user("恶寒重发热轻、无汗"));
    let mixed = assess(&res, &user("恶寒重发热轻、无汗，但又有汗"));

    let clean_p = clean.primary.expect("纯风寒语料应辨出主证");
    let mixed_p = mixed.primary.expect("含矛盾表现时仍应辨出主证");
    assert!(
        mixed_p.conflicting.contains(&"有汗".to_string()),
        "语料同时出现「有汗」时，应记为「无汗」的矛盾证据：{:?}",
        mixed_p.conflicting
    );
    assert!(
        mixed_p.confidence < clean_p.confidence,
        "矛盾证据应降低置信度：{} 应小于 {}",
        mixed_p.confidence,
        clean_p.confidence
    );
}

// ---------------- T4.2 兼证 ----------------

#[test]
fn mixed_corpus_reports_concurrent_syndromes() {
    let res = bundle();
    // 脾胃湿热（口苦口臭、肢体困重、食欲不振）+ 肝郁气滞（胸胁胀闷、烦躁易怒）
    let r = assess(
        &res,
        &user("口苦、口臭、肢体困重、食欲不振、脘腹胀满，又胸胁胀闷、烦躁易怒、善太息"),
    );

    let primary = r.primary.expect("混合语料应辨出主证");
    assert!(
        !r.concurrent.is_empty(),
        "两类证候证据相当时不应只报主证：{primary:?}"
    );
    let names: Vec<String> = r
        .concurrent
        .iter()
        .map(|c| c.name.clone())
        .chain(std::iter::once(primary.name.clone()))
        .collect();
    assert!(
        names.contains(&"脾胃湿热".to_string()) && names.contains(&"肝郁气滞".to_string()),
        "主证与兼证应覆盖并存的两个证候：{names:?}"
    );
    // 兼证按置信度降序
    let confidences: Vec<f64> = r.concurrent.iter().map(|c| c.confidence).collect();
    let mut sorted = confidences.clone();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    assert_eq!(confidences, sorted, "兼证应按置信度降序：{confidences:?}");
}

#[test]
fn benign_corpus_yields_no_primary_syndrome() {
    let res = bundle();
    let r = assess(&res, &user("你好"));
    assert!(r.primary.is_none(), "无四诊证据时不应凭空给出证候");
    assert!(r.concurrent.is_empty());
}

#[test]
fn assessment_is_deterministic_and_renders_markdown() {
    let res = bundle();
    let msgs = user("恶寒重发热轻、无汗、流清涕");
    let a = assess(&res, &msgs);
    let b = assess(&res, &msgs);
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        serde_json::to_value(&b).unwrap(),
        "同一份语料必须得到同一份结构化结论"
    );

    let md = a.render();
    assert!(md.contains("【结构化辨证】"), "渲染结果缺少标题：{md}");
    assert!(md.contains("置信度"), "渲染结果缺少置信度：{md}");
    assert!(md.contains("支持证据"), "渲染结果缺少支持证据：{md}");
}

#[test]
fn diagnosis_payload_exposes_structured_differentiation() {
    let res = bundle();
    let r = assess(
        &res,
        &user("口苦、口臭、肢体困重、食欲不振、胸胁胀闷、烦躁易怒"),
    );
    let d = Diagnosis {
        steps: vec![(Capability::Differentiation, r.render())],
        final_text: "summary".to_string(),
        failures: vec![],
        skipped: vec![],
        blocked: None,
        trace: vec![],
        structured: vec![(
            Capability::Differentiation,
            serde_json::to_value(&r).unwrap(),
        )],
    };

    let v = diagnosis_payload(&d);
    let name = v["structured"]["differentiation"]["primary"]["name"]
        .as_str()
        .unwrap_or_default();
    assert!(!name.is_empty(), "结构化输出应按 capability 键暴露：{v}");
    assert!(
        v["structured"]["differentiation"]["primary"]["confidence"]
            .as_f64()
            .unwrap_or_default()
            > 0.0
    );
    assert!(v["structured"]["differentiation"]["concurrent"]
        .as_array()
        .is_some());
}

// ---------------- T4.5 MCP Server ----------------

use harness::mcp::server::{self, LIST_CAPABILITIES_TOOL, PROTOCOL_VERSION, RUN_AGENT_TOOL};

#[test]
fn mcp_exposes_one_tool_per_capability_plus_two_entries() {
    let tools = server::tool_definitions();
    let names: Vec<String> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap_or_default().to_string())
        .collect();

    assert_eq!(
        tools.len(),
        Capability::ALL.len() + 2,
        "7 个 agent_* 工具 + run_agent + list_agent_capabilities：{names:?}"
    );
    for cap in Capability::ALL {
        assert!(
            names.contains(&server::agent_tool_name(cap)),
            "缺少 {cap:?} 对应工具：{names:?}"
        );
    }
    assert!(names.contains(&RUN_AGENT_TOOL.to_string()));
    assert!(names.contains(&LIST_CAPABILITIES_TOOL.to_string()));

    // 入参 schema 必须声明 messages，否则客户端无从传上下文
    let diff = tools
        .iter()
        .find(|t| t["name"] == "agent_differentiation")
        .expect("缺少辨证工具");
    let required = diff["inputSchema"]["required"].as_array().unwrap();
    assert!(
        required.iter().any(|r| r == "messages"),
        "入参应要求 messages：{diff}"
    );
}

#[tokio::test]
async fn mcp_initialize_and_tools_list_follow_jsonrpc() {
    let st = app_state().await;

    let init = server::handle(&st, &json!({"jsonrpc":"2.0","id":1,"method":"initialize"}))
        .await
        .expect("initialize 应有响应");
    assert_eq!(init["id"], json!(1));
    assert_eq!(init["result"]["protocolVersion"], json!(PROTOCOL_VERSION));
    assert_eq!(init["result"]["serverInfo"]["name"], json!("tcm-harness"));

    let list = server::handle(
        &st,
        &json!({"jsonrpc":"2.0","id":"x","method":"tools/list"}),
    )
    .await
    .expect("tools/list 应有响应");
    assert_eq!(list["id"], json!("x"));
    let tools = list["result"]["tools"].as_array().expect("tools 应为数组");
    assert!(!tools.is_empty());
}

#[tokio::test]
async fn mcp_unknown_method_returns_jsonrpc_error() {
    let st = app_state().await;
    let r = server::handle(&st, &json!({"jsonrpc":"2.0","id":7,"method":"foo/bar"}))
        .await
        .expect("未知方法也应回包（带 error）");
    assert_eq!(r["error"]["code"], json!(server::METHOD_NOT_FOUND));
    assert!(r.get("result").is_none());
}

#[tokio::test]
async fn mcp_notification_gets_no_response() {
    let st = app_state().await;
    let r = server::handle(
        &st,
        &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
    assert!(r.is_none(), "通知不应回包（路由层返回 204）");
}

#[tokio::test]
async fn mcp_list_capabilities_does_not_need_llm() {
    let st = app_state().await;
    let r = server::handle(
        &st,
        &json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name": LIST_CAPABILITIES_TOOL, "arguments":{}}
        }),
    )
    .await
    .expect("tools/call 应有响应");

    assert_eq!(r["result"]["isError"], json!(false));
    let text = r["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    for cap in Capability::ALL {
        assert!(text.contains(cap.slug()), "清单应含 {cap:?}：{text}");
    }
}

#[tokio::test]
async fn mcp_unknown_tool_is_rejected_at_protocol_level() {
    let st = app_state().await;
    let r = server::handle(
        &st,
        &json!({
            "jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"agent_not_exist","arguments":{"messages":[]}}
        }),
    )
    .await
    .expect("未知工具应回 JSON-RPC error");
    assert_eq!(r["error"]["code"], json!(server::INVALID_PARAMS));
}

#[tokio::test]
async fn mcp_run_agent_requires_messages() {
    let st = app_state().await;
    // 参数校验在调用 LLM 之前完成，因此本用例不需要真实 LLM
    let r = server::handle(
        &st,
        &json!({
            "jsonrpc":"2.0","id":4,"method":"tools/call",
            "params":{"name":"agent_differentiation","arguments":{"messages":[]}}
        }),
    )
    .await
    .expect("缺 messages 应回 JSON-RPC error");
    assert_eq!(r["error"]["code"], json!(server::INVALID_PARAMS));
}
