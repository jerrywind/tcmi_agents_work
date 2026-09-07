//! 流式（SSE）层的确定性回归
//!
//! 这一层错了不会报错：帧编不出来、通道没关、增量没带下标，
//! 表现都是「界面停在那里」或「四段文字混在一起」——**静默失效**。
//! 真机一次验证要 100–500 秒，代价远高于这几条断言。
//!
//! 运行：`cargo test -p harness --test stream`（容器内）

use harness::model::{Capability, Message};
use harness::orchestrator::understood_symptoms;
use harness::resources::load;
use harness::resources::model::ResourceBundle;
use harness::stream::{
    plan_item, DeltaSink, StreamEvent, StreamSink, EV_STEP_DELTA, EV_STEP_DONE, EV_STEP_RETRY,
};
use serde_json::json;
use std::path::Path;
use tokio::sync::mpsc;

fn recv<T>(rx: &mut mpsc::UnboundedReceiver<T>) -> Option<T> {
    rx.try_recv().ok()
}

#[test]
fn frame_is_three_lines_and_json_is_single_line() {
    // 正文里带换行也必须压成一行：否则 `data:` 会被拆成多行，
    // 前端按行解析时拿到半截 JSON。
    let ev = StreamEvent::new(EV_STEP_DONE, json!({ "text": "第一行\n第二行" }));
    let frame = ev.encode();
    assert!(
        frame.starts_with("event: step_done\ndata: "),
        "帧头固定两行：{frame:?}"
    );
    assert!(frame.ends_with("\n\n"), "帧尾必须是空行：{frame:?}");
    // `lines()` 会给结尾的空行留一个空串，故先滤掉再看
    let lines: Vec<&str> = frame.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 2, "data 恒为单行，故整帧只有两行：{lines:?}");
    assert!(lines[1].contains("\\n"), "换行应被转义：{}", lines[1]);
}

#[test]
fn disabled_sink_drops_everything_and_is_never_closed() {
    let sink = StreamSink::disabled();
    assert!(!sink.is_enabled());
    sink.emit(EV_STEP_DONE, json!({"a": 1}));
    // 关键：非流式调用（/chat、MCP、llm_eval）绝不能被判定为「已断开」，
    // 否则编排器会提前终止，返回一份残缺报告。
    assert!(!sink.is_closed(), "从未连接的 sink 不算断开");
}

#[test]
fn sink_emits_then_detects_disconnect() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let sink = StreamSink::new(tx);
    assert!(sink.is_enabled());
    assert!(!sink.is_closed());

    sink.emit(EV_STEP_DONE, json!({ "index": 0 }));
    let ev = recv(&mut rx).expect("应收到事件");
    assert_eq!(ev.event, EV_STEP_DONE);
    assert_eq!(ev.data["index"], 0);

    // 客户端断开：接收端释放后继续 emit 不应 panic，
    // 且 `is_closed()` 要翻转为 true（编排器据此停止后续 LLM 调用）。
    drop(rx);
    sink.emit(EV_STEP_DONE, json!({ "index": 1 }));
    assert!(sink.is_closed(), "断连后应可检测");
    // 再次 emit 只是静默丢弃，不 panic
    sink.emit(EV_STEP_DONE, json!({ "index": 2 }));
}

#[test]
fn delta_sink_carries_index_and_capability() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    // 采集期四步并行，增量会交织；必须带 index/capability 才能归位
    let delta = DeltaSink::new(StreamSink::new(tx), 3, Capability::Palpation);
    delta.push("脉");
    delta.push("浮");
    delta.push(""); // 空片不发帧，避免刷屏

    let a = recv(&mut rx).expect("第一片");
    assert_eq!(a.event, EV_STEP_DELTA);
    assert_eq!(a.data["index"], 3);
    assert_eq!(a.data["capability"], "palpation");
    assert_eq!(a.data["delta"], "脉");

    let b = recv(&mut rx).expect("第二片");
    assert_eq!(b.data["delta"], "浮");
    assert!(recv(&mut rx).is_none(), "空片不应产生事件");
}

// ---------------- 「我读到的表现」（hello 首帧的 understood 字段） ----------------

fn bundle() -> ResourceBundle {
    load(Path::new("resources")).expect("资源加载失败")
}

fn user_msgs(text: &str) -> Vec<Message> {
    vec![Message {
        role: "user".to_string(),
        content: text.to_string(),
    }]
}

#[test]
fn understood_extracts_rule_matched_manifestations() {
    let res = bundle();
    let tags = understood_symptoms(&res, &user_msgs("咳嗽三天，痰黄稠，咽痛，舌红苔黄，脉浮数"));
    assert!(!tags.is_empty(), "典型主诉必须能抽出表现：{tags:?}");
    // 抽出的是**原文里出现过的**词，不是模型概括——这是它可信的前提
    let joined = tags.join("|");
    assert!(
        joined.contains("咳") || joined.contains("痰"),
        "应含症状类证据：{tags:?}"
    );
    assert!(tags.len() <= 12, "要有上限，否则首帧会塞爆：{}", tags.len());

    // 每条都必须是**患者原话里出现过的词**。
    // `keywords.yaml` 的证据标签（如「胃火炽盛证据」）是系统贴的推断标签，
    // 混进「我读到的表现」会让用户以为自己说过——那是把推断伪装成事实。
    let text = "咳嗽三天，痰黄稠，咽痛，舌红苔黄，脉浮数";
    for t in &tags {
        let term = t.split('：').next_back().unwrap_or(t.as_str());
        assert!(
            text.contains(term),
            "「{t}」不是患者原话（关键词证据标签不得混入）：{tags:?}"
        );
    }
}

#[test]
fn understood_is_empty_when_nothing_matches() {
    let res = bundle();
    // 完全无关的输入不该硬凑出「读到的表现」：宁可空着，也不要误导
    let tags = understood_symptoms(&res, &user_msgs("我今天中午吃了三个包子"));
    assert!(tags.is_empty(), "无匹配时必须为空：{tags:?}");
}

#[test]
fn plan_item_shape_is_stable() {
    let p = plan_item(2, Capability::Safety, "safety");
    assert_eq!(p["index"], 2);
    assert_eq!(p["capability"], "safety");
    assert_eq!(p["zh"], "安全门");
    assert_eq!(p["phase"], "safety");
}

#[test]
fn retry_frame_carries_index_and_attempt() {
    // 真机实测的教训：单次超时 120s × 最多 3 次尝试 = 6 分钟里客户端
    // 一个字都收不到，只有心跳；而重试会把正文从头再推一遍，
    // 前端按 index 累加就拼出两份。这帧必须带上「第几次」与「下标」。
    let (tx, mut rx) = mpsc::unbounded_channel();
    let d = DeltaSink::new(StreamSink::new(tx), 3, Capability::Prescription);
    d.emit_retry(2, 2, "读取 LLM 流式响应失败");

    let ev = recv(&mut rx).expect("重试必须推一帧");
    assert_eq!(
        ev.event, EV_STEP_RETRY,
        "事件名错了前端收不到：{}",
        ev.event
    );
    assert_eq!(ev.data["index"], 3, "不带下标前端无法归位");
    assert_eq!(ev.data["capability"], "prescription");
    assert_eq!(ev.data["attempt"], 2);
    assert_eq!(ev.data["max_retries"], 2);
    assert!(ev.data["error"].as_str().unwrap().contains("流式"));
}
