//! 行为回归：T2.x / T3.x 新增能力的**确定性**部分
//!
//! 需要 LLM 的链路不在此覆盖（harness 无 MockProvider）：
//! 这里只验证纯函数与数据结构契约——红旗中断判定、技能归属、埋点累加、
//! 配置解析，以及 `/chat` 响应的 `blocked` / `skipped` / `trace` 字段。
//!
//! 运行：`cargo test -p harness --test behavior`（容器内）

use harness::agents::acupuncture::syndrome_block;
use harness::agents::case_reference::infer_syndromes_hint;
use harness::agents::convergence::{evaluate, LoopConfig};
use harness::agents::differentiation::assess;
use harness::agents::{blocking_red_flag, detect_red_flags, is_blocking, resolve_syndrome};
use harness::config::HarnessConfig;
use harness::knowledge::{check_composition, find_formula};
use harness::model::{Capability, Message};
use harness::orchestrator::{
    build_confidence_note, diagnosis_payload, lock_syndrome, resolve_order, safety_corpus,
    split_phases, with_rag_status, Blocked, Diagnosis, SyndromeLock, DISCLAIMER,
    LOCK_MIN_CONFIDENCE,
};
use harness::resources::load;
use harness::resources::model::{Gender, ResourceBundle, Routing};
use harness::skills::build_default_registry;
use harness::trace::{new_trace, record, snapshot, LlmCallStat, StepTrace};
use harness::AppState;
use serde_json::json;
use std::path::{Path, PathBuf};

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
    // 末位是编排器与技能共享的「当前科室」，测试里用空列表即可
    let reg = build_default_registry(
        &cfg,
        &res,
        reqwest::Client::new(),
        std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    );

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
    // 13 个：四诊 4 + 医案参考 + 辨证 + 安全门 + 立法 / 用药 / 开方 + 调护 / 针灸 + 治疗
    assert_eq!(Capability::ALL.len(), 13);
    // 新增的治疗期三步：slug 与中文名都要能解析（前端按中文名过滤技能 owner）
    assert_eq!(
        Capability::from_name("prescription"),
        Some(Capability::Prescription)
    );
    assert_eq!(
        Capability::from_name("开方"),
        Some(Capability::Prescription)
    );
    assert_eq!(Capability::COLLECTION.len(), 4);
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
        ..Default::default()
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
    // H1：关键词证据**不得**与症状重复计分。
    //
    // 本语料命中的关键词（恶寒 / 无汗 / 清涕 / 头痛 / 浮紧 …）全部是
    // 症状表或脉象字段里已有原词的子串，理应被去重跳过。
    // 若这里又出现「风寒证据」标签，说明「同一表现算两遍」的老毛病回来了——
    // 那会把证据量放大约 1.5 倍，置信度跟着虚高。
    assert!(
        !primary.supporting.contains(&"风寒证据".to_string()),
        "症状已命中的关键词不应重复计入支持证据（H1）：{:?}",
        primary.supporting
    );
    // 4 条主症 4.0 + 2 条次症 0.8 + 脉象 1.0 = 5.8（重复计分时会是 8.8）
    assert!(
        (primary.score - 5.8).abs() < 1e-9,
        "证据量应恰为 5.8，实得 {}——关键词被重复计分了",
        primary.score
    );
    assert!(primary.conflicting.is_empty(), "无相反表现时不应有矛盾证据");
    assert!(primary.pathogenesis.is_some(), "主证应带病机");
}

/// H1 的另一面：症状表**之外**的同义说法仍要计入，否则关键词证据就废了
#[test]
fn keyword_evidence_still_credits_clues_outside_the_symptom_list() {
    let res = bundle();
    // 「喉中痰鸣」「痰多易咯」只在 keywords.yaml 里，症状表里没有；
    // 「咳嗽痰多」「痰白黏」「胸闷」「气喘」是痰湿阻肺的四条主症。
    let r = assess(
        &res,
        &user("咳嗽痰多，痰白黏，胸闷，气喘，喉中痰鸣，痰多易咯"),
    );
    let primary = r.primary.expect("主症四条全中应足以定证");
    assert_eq!(primary.slug, "phlegm_damp_obstructing_lung");
    assert!(
        primary.supporting.contains(&"痰湿阻肺证据".to_string()),
        "症状表之外的关键词应计入支持证据：{:?}",
        primary.supporting
    );
}

/// H2：只凑次症压不过命中主症——主症权重必须实打实高于次症
#[test]
fn minor_symptoms_cannot_outweigh_key_symptoms() {
    let res = bundle();
    let r = assess(&res, &user("乏力，面色萎黄"));
    let candidate = r
        .ranked
        .iter()
        .find(|s| s.slug == "dampness_encumbering_spleen")
        .expect("次症命中应进候选集");
    assert!(
        !candidate.qualified,
        "只中次症不满足主症必备（H3）：{:?}",
        candidate
    );
    assert!(
        (candidate.score - 0.8).abs() < 1e-9,
        "两条次症应为 2 × 0.4 = 0.8，实得 {}",
        candidate.score
    );
    assert!(r.primary.is_none(), "只凑次症不得出主证");
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
        ..Default::default()
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

// ---------------- T5.1 报告持久化 / T5.4 合规 ----------------

/// 起一个真实监听的 harness（随机端口），返回 base url 与进程内状态
async fn spawn_app(store_dir: Option<PathBuf>) -> (String, AppState) {
    spawn_app_with(HarnessConfig {
        resources_dir: PathBuf::from("resources"),
        store_dir,
        ..HarnessConfig::default()
    })
    .await
}

async fn spawn_app_with(cfg: HarnessConfig) -> (String, AppState) {
    let st = AppState::load(cfg).await.expect("AppState 加载失败");
    let app = harness::http::build_router(st.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定随机端口失败");
    let addr = listener.local_addr().expect("取监听地址失败");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("服务异常退出");
    });
    (format!("http://{addr}"), st)
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "harness-behavior-{tag}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).expect("创建临时目录失败");
    d
}

#[tokio::test]
async fn reports_can_be_archived_and_retrieved() {
    let dir = temp_dir("reports");
    let (base, st) = spawn_app(Some(dir.clone())).await;

    // 归档：走 store 直接写入（/chat 需要 LLM，端到端由人工验收覆盖）
    let id = st
        .store
        .save(
            &json!({"steps": [{"capability":"safety"}], "summary": "ok", "partial": false}),
            &json!([{"role":"user","content":"口苦口臭，手机 13812345678"}]),
            &json!({"age": 34}),
        )
        .expect("落盘失败")
        .expect("启用存储后应返回 id");

    // 回查：内容一致，且入参已脱敏
    let got: serde_json::Value = reqwest::get(format!("{base}/reports/{id}"))
        .await
        .expect("请求失败")
        .json()
        .await
        .expect("解析失败");
    assert_eq!(got["id"], json!(id));
    assert_eq!(got["result"]["summary"], json!("ok"));
    assert_eq!(got["payload"]["age"], json!(34));
    let stored_msg = got["messages"][0]["content"].as_str().unwrap_or_default();
    assert!(
        stored_msg.contains("口苦口臭"),
        "症状描述应保留：{stored_msg}"
    );
    assert!(
        stored_msg.contains("[手机号已脱敏]"),
        "落盘应脱敏：{stored_msg}"
    );

    // 列表：最新一份即刚才那份
    let list: serde_json::Value = reqwest::get(format!("{base}/reports"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["enabled"], json!(true));
    assert_eq!(list["reports"][0]["id"], json!(id));
    assert_eq!(list["reports"][0]["steps"], json!(1));

    // 不存在的 id → 404（而非 500 或空 200）
    let miss = reqwest::get(format!("{base}/reports/does-not-exist"))
        .await
        .unwrap();
    assert_eq!(miss.status(), 404);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn reports_endpoints_degrade_gracefully_when_store_disabled() {
    // 默认配置不启用持久化：端点必须明确告知「未启用」，而不是返回空列表假装成功
    let (base, st) = spawn_app(None).await;
    assert!(!st.store.is_enabled());

    let list: serde_json::Value = reqwest::get(format!("{base}/reports"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["enabled"], json!(false));
    assert!(list["reports"].as_array().unwrap().is_empty());

    let r = reqwest::get(format!("{base}/reports/anything"))
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

#[test]
fn safety_step_cannot_be_removed_by_routing() {
    // 合规底线（T5.4）：红旗路径不可移除。
    // routing.yaml 允许增删步骤，但把 safety 删掉会让红旗症状直接走到治疗建议。
    let res = ResourceBundle {
        routing: Routing {
            active: vec!["differentiation".to_string(), "treatment".to_string()],
            default: None,
            ..Default::default()
        },
        ..ResourceBundle::default()
    };
    let order = resolve_order(&res);
    assert!(
        order.contains(&Capability::Safety),
        "安全门必须被强制补齐：{order:?}"
    );
    let pos_safety = order
        .iter()
        .position(|c| *c == Capability::Safety)
        .expect("含 safety");
    let pos_treatment = order
        .iter()
        .position(|c| *c == Capability::Treatment)
        .expect("含 treatment");
    assert!(
        pos_safety < pos_treatment,
        "安全门必须排在治疗之前：{order:?}"
    );

    // 已显式配置时保持原样，不重复插入
    let res2 = ResourceBundle {
        routing: Routing {
            active: vec!["safety".to_string(), "treatment".to_string()],
            default: None,
            ..Default::default()
        },
        ..ResourceBundle::default()
    };
    assert_eq!(
        resolve_order(&res2),
        vec![Capability::Safety, Capability::Treatment]
    );
}

#[test]
fn chat_payload_carries_disclaimer() {
    // 合规（T5.4）：免责声明随每份结果下发，接入方不必（也不该）自己编一段
    let d = Diagnosis {
        steps: vec![(Capability::Safety, "未触发红色警戒".to_string())],
        final_text: "x".to_string(),
        failures: vec![],
        skipped: vec![],
        blocked: None,
        trace: vec![],
        structured: vec![],
        ..Default::default()
    };
    let v = diagnosis_payload(&d);
    assert_eq!(v["disclaimer"], json!(DISCLAIMER));
    // 未启用 loop 时不该出现 awaiting_input，避免调用方误以为还要继续追问
    assert_eq!(v["status"], json!("completed"));
    assert!(
        v["disclaimer"].as_str().unwrap().contains("不构成医疗诊断"),
        "免责声明内容应完整：{}",
        v["disclaimer"]
    );
}

// ---------------- T7.7 安全门先于收敛判定 ----------------

#[test]
fn safety_has_its_own_phase_before_convergence() {
    let res = bundle();
    let order = resolve_order(&res);
    let (collection, diagnosis, post) = split_phases(&order);

    // 安全门曾归属治疗期，于是排在收敛判定**之后**：信息不足以辨证时流程会
    // 停下来追问（awaiting_input），安全门整个被跳过。真实验证里
    // 「突然胸痛剧烈、出冷汗、呼吸困难、左臂发麻」这条典型心梗表现因此漏检。
    assert!(
        !post.contains(&Capability::Safety),
        "安全门不得归属治疗期：会排在收敛判定之后而被整个跳过"
    );
    assert!(
        !collection.contains(&Capability::Safety) && !diagnosis.contains(&Capability::Safety),
        "安全门独立成阶段，不属于采集期也不属于辨证期"
    );

    // 治疗三步仍在治疗期，阶段划分本身没有走样
    for c in [
        Capability::Strategy,
        Capability::Herbology,
        Capability::Prescription,
    ] {
        assert!(post.contains(&c), "{c:?} 应属治疗期：{post:?}");
    }
    assert_eq!(collection.len(), 4, "四诊仍在采集期：{collection:?}");
    assert!(diagnosis.contains(&Capability::Differentiation));
}

#[test]
fn rag_status_is_injected_into_treatment_payload() {
    use harness::rag_health::{RagStatus, SharedRagStatus};
    use std::sync::{Arc, RwLock};

    let mk = |reachable: Option<bool>| -> SharedRagStatus {
        Arc::new(RwLock::new(RagStatus {
            configured: true,
            reachable,
            endpoint: Some("http://example/rag".to_string()),
            last_error: None,
            since_last_ok_secs: None,
        }))
    };

    // 可达 → true
    let out = with_rag_status(json!({"syndrome": "x"}), &mk(Some(true)));
    assert_eq!(out["rag_available"], json!(true));

    // 明确不可达 → false（开方步据此拒绝编造书名）
    let out = with_rag_status(json!({"syndrome": "x"}), &mk(Some(false)));
    assert_eq!(out["rag_available"], json!(false));

    // 还没探测过（None）按不可用处理：宁可少引一处出处，
    // 也不要让「未经核对」冒充「有典籍支撑」
    let out = with_rag_status(json!({}), &mk(None));
    assert_eq!(out["rag_available"], json!(false));

    // 原有字段不得被冲掉
    let out = with_rag_status(
        json!({"syndrome": "spleen_stomach_damp_heat"}),
        &mk(Some(true)),
    );
    assert_eq!(out["syndrome"], json!("spleen_stomach_damp_heat"));
}

#[test]
fn safety_corpus_uses_only_patient_statements() {
    // 多轮问诊曾从第二轮起被误拦截：上一轮安全门输出的警示文案
    // （「若出现胸痛、呼吸困难…请立即就医」）被算进语料，预检必然命中，
    // 患者补答之后反而只剩 safety 一步、永远拿不到方案。
    let msgs = vec![
        Message {
            role: "user".to_string(),
            content: "口苦口臭，大便粘滞不爽".to_string(),
        },
        Message {
            role: "assistant".to_string(),
            content: "若出现胸痛、呼吸困难、咯血请立即就医".to_string(),
        },
    ];
    let corpus = safety_corpus(&msgs);
    assert!(
        !corpus.contains("胸痛"),
        "助手的警示文案不得被当成患者症状：{corpus}"
    );
    assert!(corpus.contains("口苦"), "患者陈述必须保留：{corpus}");
    assert!(
        blocking_red_flag(&bundle(), &corpus).is_none(),
        "仅凭助手文本不应触发拦截"
    );
}

#[test]
fn red_flag_corpus_is_detected_without_llm() {
    // 预检是纯函数：不必等四诊跑完就能判定，这是「跳过采集直接拦截」的前提
    let res = bundle();
    let corpus = "突然胸痛剧烈，出冷汗，呼吸困难，左臂发麻";
    let rf = blocking_red_flag(&res, corpus).expect("典型心梗表现必须被识别为红旗");
    assert!(is_blocking(rf), "该红旗必须触发中断：{}", rf.label);
}

// ---------------- T7.5 RAG 可达性 ----------------

async fn get_json(base: &str, path: &str) -> serde_json::Value {
    reqwest::get(format!("{base}{path}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// 探测是后台任务发起的，轮询等它落地（最多 5 秒）
async fn wait_until_rag_probed(base: &str) -> serde_json::Value {
    for _ in 0..50 {
        let v = get_json(base, "/health").await;
        if !v["rag"]["reachable"].is_null() {
            return v;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("RAG 探测结果未在 5 秒内写入 /health")
}

#[tokio::test]
async fn health_status_stays_ok_and_reports_rag_state() {
    // 进程存活与否，和「典籍检索接没接上」是两件事：
    // 后者故障时不能把整个服务标记为不健康，否则编排会误判需要重启。
    let (base, _st) = spawn_app(None).await;
    let v = get_json(&base, "/health").await;
    assert_eq!(v["status"], json!("ok"), "/health 仍应报告进程存活：{v}");
    // 默认的 HarnessConfig 不带 rag_endpoint
    assert_eq!(
        v["rag"]["configured"],
        json!(false),
        "未配置应如实报告：{v}"
    );
    assert!(
        v["rag"]["reachable"].is_null(),
        "未配置端点时不应给出可达性结论：{v}"
    );
}

#[tokio::test]
async fn unreachable_rag_is_reported_instead_of_silently_degrading() {
    // 单机 docker run 时 rag_endpoint（compose 服务名）解析不到，
    // 技能若静默降级，「没检索到典籍」就会长得跟「典籍里没有」一模一样。
    let (base, _st) = spawn_app_with(HarnessConfig {
        resources_dir: PathBuf::from("resources"),
        // 端口 1：必然连接被拒，探测会立即失败
        rag_endpoint: Some("http://127.0.0.1:1/rag/retrieve/text".to_string()),
        ..HarnessConfig::default()
    })
    .await;

    let v = wait_until_rag_probed(&base).await;
    assert_eq!(v["rag"]["configured"], json!(true));
    assert_eq!(v["rag"]["reachable"], json!(false), "不可达必须被报告：{v}");
    assert!(
        v["rag"]["last_error"].as_str().is_some(),
        "应带上失败原因便于排查：{v}"
    );
    // 从未成功过时不应给出「距上次成功」的数字
    assert!(v["rag"]["since_last_ok_secs"].is_null(), "{v}");
}

// ---------------- T7.1 证候锁定 ----------------
//
// 治疗期各步（立法 / 用药 / 开方 …）此前各自从原始对话重新猜证候，
// 与辨证步的 `assess()` 不是同一套算法，于是出现「辨证脾胃湿热、
// 开方龙胆泻肝汤」这类方证不对口。锁定后它们读到的是同一份主证。

#[test]
fn differentiation_result_is_injected_into_post_phase_payload() {
    let res = bundle();
    let msgs = user("口苦口臭，脘腹胀满，食欲不振，肢体困重，大便溏黏，小便黄，舌红苔黄腻，脉滑数");
    let (locked, lock) = lock_syndrome(&res, &msgs, &json!({}));

    assert_eq!(
        locked["syndrome"].as_str(),
        Some("spleen_stomach_damp_heat"),
        "辨证主证必须注入 payload，否则治疗期各步会各自重猜：{locked}"
    );
    assert_eq!(locked["syndrome_name"].as_str(), Some("脾胃湿热"));
    assert!(
        locked["syndrome_confidence"].as_f64().unwrap_or(0.0) > 0.0,
        "应一并带上置信度：{locked}"
    );
    let lock = lock.expect("高置信度时应返回锁定信息");
    assert!(lock.locked);
    assert_eq!(lock.slug, "spleen_stomach_damp_heat");
}

#[test]
fn explicit_syndrome_is_never_overridden() {
    let res = bundle();
    let msgs = user("口苦口臭，舌红苔黄腻");
    // 「已知证候求方剂」场景：调用方明说了证候，编排器不该用自己的结论覆盖
    let (locked, lock) = lock_syndrome(&res, &msgs, &json!({"syndrome": "wind_cold_attack_lung"}));
    assert_eq!(locked["syndrome"].as_str(), Some("wind_cold_attack_lung"));
    // 显式给定是「已知证候求方剂」的前提，视为确定，不参与置信度门槛
    let lock = lock.expect("显式给定证候时应返回锁定信息");
    assert!(lock.locked);
    assert_eq!(lock.slug, "wind_cold_attack_lung");
}

// ---------------- I1 针灸步的漏网 ----------------
//
// H6 把「治疗期各步要感知证候置信度」铺到了立法 / 用药 / 开方 / 治疗 / 调护五步，
// 漏了第六步——针灸。它的 payload 参数名叫 `_payload`，整个被丢弃：
// 辨证判定「未匹配到证候」时，针灸照样一本正经开穴。
// 而**针灸是有创操作**，未定证就给穴位处方，风险不比开错方小。
//
// 教训同 T7.12：改一个模式时，必须枚举**所有**同类调用点，
// 包括那些「默认不在激活档位」的——配置支持显式启用，用户就会踩到。

#[test]
fn acupuncture_sees_the_same_syndrome_as_the_rest_of_the_treatment_phase() {
    let res = bundle();
    let msgs = user("口苦，脘腹胀满，大便溏黏，粘滞不爽，肢体困重，舌红，苔黄腻，脉滑数");
    let locked = {
        let (p, _) = lock_syndrome(&res, &msgs, &json!({}));
        p
    };

    let block = syndrome_block(&res, &msgs, &locked);
    assert!(block.contains("脾胃湿热"), "针灸步应拿到辨证主证：{block}");
    assert!(
        block.contains("【当前主证】"),
        "取穴须以主证为据，不该凭对话自行推测：{block}"
    );
    // 病机与治则一并注入：取穴思路要与之对得上
    assert!(block.contains("【病机】"), "应带病机：{block}");
    assert!(block.contains("【治则】"), "应带治则：{block}");
}

#[test]
fn acupuncture_gets_no_syndrome_when_differentiation_could_not_decide() {
    let res = bundle();
    let msgs = user("最近有点乏力");
    let (payload, _) = lock_syndrome(&res, &msgs, &json!({}));

    assert!(
        syndrome_block(&res, &msgs, &payload).is_empty(),
        "未定证时不得给针灸步一个猜来的证——针灸是有创操作"
    );
}

// ---------------- I2 医案参考的检索靶点不得冒充结论 ----------------
//
// 本步执行在辨证**之前**，此时还没有 `assess()` 的结论，
// 靶点只能来自文本推断。措辞若不说清，模型会拿它当既成事实去检索相似医案。

#[test]
fn case_reference_target_is_labelled_as_a_preliminary_direction() {
    let res = bundle();
    let hint = infer_syndromes_hint(&res, &user("口苦口臭，舌红苔黄腻"), &json!({}));
    assert!(hint.contains("【检索靶点】"), "{hint}");
    assert!(
        hint.contains("初步方向") && hint.contains("非正式辨证结论"),
        "文本推断的靶点必须标注它不是结论：{hint}"
    );
}

#[test]
fn case_reference_drops_guessed_target_when_differentiation_said_unmatched() {
    let res = bundle();
    let payload = json!({"syndrome_matched": false});
    assert_eq!(
        infer_syndromes_hint(&res, &user("口苦口臭，舌红苔黄腻"), &payload),
        "",
        "辨证已判定未匹配时，不该再按猜的证把医案检索一遍"
    );
    // 调用方显式给的证候是明说的，不受此限
    let hint = infer_syndromes_hint(
        &res,
        &user("口苦口臭"),
        &json!({"syndrome": "脾胃湿热", "syndrome_matched": false}),
    );
    assert!(hint.contains("已知证候：脾胃湿热"), "{hint}");
}

// ---------------- H4 证候锁定的置信度门槛 ----------------
//
// 此前只要 `assess()` 产出主证就锁定，而 `assess()` 是在有限的证候库里
// 必选其一——命中一条次症就能凑出主证，于是治疗期按一个可能是错的证开方，
// 报告里却看不出这是猜的。

#[test]
fn low_confidence_syndrome_is_not_locked() {
    let res = bundle();
    // 主症「口苦」1.0 + 次症「口臭」「肢体困重」各 0.4 = 1.8
    // → 过主证门槛 1.5（可以报出证候），但置信度 0.36 未达锁定门槛 0.4
    let msgs = user("口苦，口臭，肢体困重");
    let (locked, lock) = lock_syndrome(&res, &msgs, &json!({}));

    assert!(
        locked.get("syndrome").is_none(),
        "置信度不足时不注入 syndrome，否则治疗期会按一个未核实的证开方：{locked}"
    );
    // 但必须留下痕迹，供治疗期各步与报告标注不确定
    assert_eq!(locked["syndrome_matched"], json!(true));
    assert_eq!(locked["syndrome_locked"], json!(false));
    assert_eq!(locked["syndrome_name"].as_str(), Some("脾胃湿热"));

    let lock = lock.expect("未达门槛也要带回主证信息，供报告标注");
    assert!(!lock.locked);
    assert!(lock.confidence < LOCK_MIN_CONFIDENCE);
}

#[test]
fn unmatched_syndrome_is_not_locked_and_marked() {
    let res = bundle();
    // 只中次症（乏力），无主症：库外典型的「看起来像但定不了」
    let msgs = user("最近有点乏力");
    let (locked, lock) = lock_syndrome(&res, &msgs, &json!({}));

    assert!(locked.get("syndrome").is_none());
    assert_eq!(
        locked["syndrome_matched"],
        json!(false),
        "必须显式标注未匹配，治疗期各步据此提示「未定证」：{locked}"
    );
    assert!(lock.is_none(), "未匹配时不应返回锁定信息");
}

#[test]
fn confidence_note_is_built_for_unreliable_conclusions() {
    // ① 未匹配到证候
    let note = build_confidence_note(None, None).expect("未匹配必须有提示");
    assert!(note.contains("未匹配到明确证候"), "{note}");

    // ② 匹配到但置信度不足、未锁定
    let note = build_confidence_note(
        Some(&SyndromeLock {
            slug: "spleen_stomach_damp_heat".into(),
            name: "脾胃湿热".into(),
            confidence: 0.28,
            locked: false,
        }),
        None,
    )
    .expect("未锁定必须有提示");
    assert!(note.contains("置信度偏低"), "{note}");
    assert!(note.contains("脾胃湿热"), "{note}");

    // ③ 已锁定且未强制放行 —— 不该有提示，避免对正常结论草木皆兵
    assert_eq!(
        build_confidence_note(
            Some(&SyndromeLock {
                slug: "spleen_stomach_damp_heat".into(),
                name: "脾胃湿热".into(),
                confidence: 0.86,
                locked: true,
            }),
            None,
        ),
        None
    );
}

// ---------------- H5 强制放行必须可见 ----------------
//
// 达到最大追问轮次后流程照常跑完开方，产出一份看起来正常的完整报告。
// `forced` 此前只躺在 loop 字段里，正文与 disclaimer 都没有痕迹。

#[test]
fn forced_convergence_is_reported_to_the_reader() {
    let res = bundle();
    let cfg = LoopConfig::default();
    // 主诉过简 → 必然不收敛；round 达到上限 → 强制放行
    let conv = evaluate(&res, &user("我咳嗽两天了"), &cfg, cfg.max_rounds);
    assert!(conv.forced);

    let note = build_confidence_note(
        Some(&SyndromeLock {
            slug: "wind_cold_attack_lung".into(),
            name: "风寒感冒".into(),
            confidence: 1.0,
            locked: true,
        }),
        Some(&conv),
    )
    .expect("强制放行必须有提示");
    assert!(note.contains("最大追问轮次"), "{note}");
    assert!(note.contains("覆盖率"), "{note}");
}

// ---------------- H5 鉴别度必须用真实第二名 ----------------
//
// 此前取 `concurrent.first()`，而兼证已被「score ≥ 主证×0.6」过滤过：
// 第二名一旦被滤掉，margin 就退化成主证自身分数，鉴别度判定形同虚设。

#[test]
fn margin_uses_the_real_runner_up_not_the_first_concurrent() {
    let res = bundle();
    // 主证明确、第二名远落于其后（不在兼证里）的语料
    let msgs = user("干咳少痰，咽干，鼻燥，痰黏难咯，声音嘶哑，唇干，舌红少津，苔薄，脉浮细");
    let d = assess(&res, &msgs);
    let primary = d.primary.as_ref().expect("应辨出风燥伤肺");
    assert_eq!(primary.slug, "wind_dryness_attacking_lung");

    let conv = evaluate(&res, &msgs, &LoopConfig::default(), 1);
    // 兼证为空（第二名被阈值滤掉）时，margin 必须仍是「主证 − 真实第二名」，
    // 而不是主证自身分数
    assert!(
        d.concurrent.is_empty(),
        "该语料不应有兼证，用于覆盖「第二名被滤掉」这条路径"
    );
    assert!(
        conv.margin < primary.score - 1e-9,
        "margin({}) 必须小于主证自身分数({})——否则说明用的不是真实第二名",
        conv.margin,
        primary.score
    );
    assert!(
        conv.margin > 0.0,
        "真实第二名存在时 margin 应大于 0，实得 {}",
        conv.margin
    );
}

#[test]
fn resolve_syndrome_accepts_slug_and_chinese_name() {
    let res = bundle();
    let msgs = user("口苦口臭，舌红苔黄腻");

    assert_eq!(
        resolve_syndrome(&res, &msgs, &json!({"syndrome": "脾胃湿热"})).as_deref(),
        Some("spleen_stomach_damp_heat"),
        "中文名应归一化成 slug，否则按 slug 查方剂会静默查不到"
    );
    assert_eq!(
        resolve_syndrome(
            &res,
            &msgs,
            &json!({"syndrome": "spleen_stomach_damp_heat"})
        )
        .as_deref(),
        Some("spleen_stomach_damp_heat")
    );
    // 证候库里没有的取值不该被静默吞掉：退回文本推断，而不是返回 None 让下游空转
    assert_eq!(
        resolve_syndrome(&res, &msgs, &json!({"syndrome": "不存在的证"})).as_deref(),
        Some("spleen_stomach_damp_heat"),
        "未知取值应回退到文本推断"
    );
}

/// H3 配套：辨证已判定「未匹配」时，治疗期不得再兜底猜一个证出来。
///
/// 否则规则层会把「【证候】X /【治则】Y」拼进输出——正文开头写着
/// 「本次未辨出明确证候」，末尾却摆着一个具体的证与治则，自相矛盾，
/// 后续用药、开方还会跟着这个猜错的证一路跑偏。
#[test]
fn unmatched_syndrome_disables_fallback_inference() {
    let res = bundle();
    let msgs = user("口苦口臭，舌红苔黄腻");
    assert_eq!(
        resolve_syndrome(&res, &msgs, &json!({"syndrome_matched": false})),
        None,
        "未匹配时不得兜底猜测证候"
    );
    // 未给该标记时（单步调用场景）仍走文本推断，保持既有行为
    assert!(
        resolve_syndrome(&res, &msgs, &json!({})).is_some(),
        "单步调用未给证候时应照常兜底推断"
    );
}

// ---------------- T7.2 技能多归属 ----------------

#[test]
fn treatment_steps_share_formula_and_care_tools() {
    let res = bundle();
    let cfg = HarnessConfig::default();
    let reg = build_default_registry(
        &cfg,
        &res,
        reqwest::Client::new(),
        std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    );
    let names = |c: Capability| -> Vec<String> {
        reg.for_capability(c)
            .iter()
            .map(|s| s.name.clone())
            .collect()
    };

    // 默认 standard 档把治疗拆成「立法 → 用药 → 开方」，
    // 拆完之后方剂检索必须对用药、开方可见——否则开方只能凭记忆写药味。
    for cap in [
        Capability::Prescription,
        Capability::Herbology,
        Capability::Treatment,
    ] {
        assert!(
            names(cap).contains(&"tcm-formula".to_string()),
            "{cap:?} 应能调用方剂检索：{:?}",
            names(cap)
        );
    }
    for cap in [Capability::Care, Capability::Treatment] {
        assert!(
            names(cap).contains(&"tcm-care".to_string()),
            "{cap:?} 应能调用调护检索：{:?}",
            names(cap)
        );
    }
    // 采集期仍看不到治疗期专属工具；全局工具对所有人可见
    let inspection = names(Capability::Inspection);
    assert!(
        !inspection.contains(&"tcm-formula".to_string()),
        "专属工具泄漏到采集期：{inspection:?}"
    );
    assert!(
        inspection.contains(&"tcm-kb".to_string()),
        "全局工具应对所有 capability 可见：{inspection:?}"
    );
}

#[tokio::test]
async fn skills_endpoint_exposes_multi_owner_tools() {
    // HTTP 层兜住 T7.2：owner 从单个 capability 改成集合后，
    // `/skills?owner=` 的过滤与展示都得跟着对，否则前端按步骤过滤工具会漏。
    let (base, _st) = spawn_app(None).await;
    let v: serde_json::Value = reqwest::get(format!("{base}/skills?owner=prescription"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let list = v["skills"].as_array().expect("应返回技能数组");
    let names: Vec<&str> = list.iter().filter_map(|s| s["name"].as_str()).collect();
    assert!(
        names.contains(&"tcm-formula"),
        "开方步应能看到方剂工具：{names:?}"
    );

    let get = |n: &str| {
        list.iter()
            .find(|s| s["name"] == n)
            .unwrap_or_else(|| panic!("缺少技能 {n}"))
    };
    let owner = get("tcm-formula")["owner"].as_str().unwrap_or("");
    assert!(
        owner.contains("开方") && owner.contains("用药"),
        "多归属应把全部可用步骤列出来：{owner}"
    );
    assert_eq!(get("tcm-kb")["owner"].as_str(), Some("全局"));
}

// ---------------- T7.3 人群适配问诊 ----------------

#[test]
fn gender_parses_chinese_and_english_payload() {
    assert_eq!(Gender::from_payload(&json!({"gender": "男"})), Gender::Male);
    assert_eq!(
        Gender::from_payload(&json!({"gender": "女性"})),
        Gender::Female
    );
    assert_eq!(
        Gender::from_payload(&json!({"gender": "female"})),
        Gender::Female
    );
    assert_eq!(Gender::from_payload(&json!({})), Gender::Unknown);
}

#[test]
fn menstruation_question_is_skipped_for_male_patients() {
    let res = bundle();
    let q = res
        .questions
        .iter()
        .find(|q| q.slug == "menstruation")
        .expect("问题库应有月经条目");

    assert!(
        !q.applies_to_gender(Gender::Male),
        "男患者不应被追问月经（人工验收点名）：{q:?}"
    );
    assert!(q.applies_to_gender(Gender::Female));
    assert!(
        q.applies_to_gender(Gender::Unknown),
        "性别未采集时不过滤：宁可多问，也不要漏掉妇科鉴别线索"
    );

    // 常规问题不受人群限制
    let fever = res
        .questions
        .iter()
        .find(|q| q.slug == "fever")
        .expect("问题库应有寒热条目");
    assert!(fever.applies_to_gender(Gender::Male));
    assert!(fever.applies_to_gender(Gender::Female));
}

// ---------------- T7.4 方剂库与药味校验 ----------------

#[test]
fn every_syndrome_has_formulas() {
    let res = bundle();
    for s in &res.syndromes {
        let f = find_formula(&res, &s.slug);
        assert!(
            !f.is_empty(),
            "证候「{}」({}) 没有适用方剂，模型只能凭记忆开方",
            s.name,
            s.slug
        );
    }
}

#[test]
fn composition_check_flags_missing_herbs_without_false_alarms() {
    let res = bundle();

    // 库载麻黄汤 = 麻黄、桂枝、杏仁、甘草：漏掉甘草必须被提示
    let notes = check_composition(&res, "拟麻黄汤：麻黄、桂枝、杏仁");
    assert!(
        notes.iter().any(|n| n.contains("甘草")),
        "漏味应被提示：{notes:?}"
    );

    // 组成一致时不得误报
    assert!(
        check_composition(&res, "拟麻黄汤：麻黄、桂枝、杏仁、甘草").is_empty(),
        "药味齐全时不应报警"
    );

    // 只提方名、没在列药味时不校验（避免「可考虑麻黄汤加减」被误判）
    assert!(
        check_composition(&res, "可考虑麻黄汤加减，随证化裁").is_empty(),
        "仅提及方名不应触发校验"
    );
}
