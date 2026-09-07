//! 流式诊断（SSE，Server-Sent Events）
//!
//! ## 为什么需要它
//!
//! `POST /chat` 是「跑完全部步骤再一次性返回」：标准档 10 步实测 200–530 秒，
//! 期间前端只有一句 `showLoading`。用户在这几分钟里**得不到任何反馈**，
//! 既不知道进行到哪一步，也不知道还要等多久——感知延迟等于真实延迟。
//!
//! 本模块把一次诊断拆成**事件流**逐步推送：
//!
//! ```text
//! hello（步骤计划，零成本）→ step_start/step_done（逐步）→ loop（收敛判定）
//!   → confidence（可信度）→ summary → done
//! ```
//!
//! ## 为什么是 SSE 而不是 WebSocket
//!
//! - `/chat` 是「一个请求 → N 次推送 → 结束」，单向即可，WebSocket 是过度设计；
//! - SSE 走普通 HTTP，nginx 只需关掉缓冲，无需额外的协议升级与保活；
//! - 小程序端没有 `EventSource`，但 `wx.request({enableChunked:true})` 能拿到分片，
//!   而 SSE 的帧格式（`data: {...}\n\n`）按行解析即可——**两端可共用一套解析器**。
//!
//! ## 帧格式约定
//!
//! ```text
//! event: <name>\n
//! data: <单行 JSON>\n
//! \n
//! ```
//!
//! `data` 恒为一行：`serde_json` 会把正文里的换行转义成 `\n`，
//! 因此不会出现「一帧被拆成多行」的歧义。另每 15 秒发一帧 `: ping\n\n` 心跳，
//! 用于穿透中间层并让前端区分「还在算」与「已经死了」。
//!
//! ## 事件一览
//!
//! | 事件 | 时机 | 用途 |
//! |---|---|---|
//! | `hello` | 请求到达即发 | 步骤计划，前端立刻渲染骨架 |
//! | `step_start` / `step_done` / `step_fail` | 每步 | 逐步点亮与逐步披露 |
//! | `red_flag` | 安全门预检命中 | 立即拦截提示（详见下） |
//! | `blocked` | 安全门确认拦截 | 置顶拦截条 |
//! | `skipped` | 拦截后 | 未执行步骤灰显 |
//! | `loop` | 收敛判定后 | 待补问题标签 |
//! | `confidence` | 证候锁定后 | 可信度横幅 |
//! | `summary` / `done` | 结束 | 结论与归档 id |
//! | `error` | 整体失败 | 前端报错 |
//!
//! `red_flag` 与 `blocked` 是两个**不同时刻**：前者是纯关键词预检（零延迟），
//! 后者是安全门 agent 跑完后的确认。两者数据一致，前端渲染同一条横幅，
//! 后到的覆盖先到的，不会闪烁。
//!
//! ## 断连即停
//!
//! 客户端断开后 `UnboundedSender::send` 会失败，[`StreamSink`] 随即置空通道。
//! 编排器在步骤之间检查 [`StreamSink::is_closed`]，已断开则停止后续步骤——
//! 用户关掉页面后不该继续烧 9 次 LLM 调用。

use axum::body::{Body, Bytes};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::model::Capability;

/// 心跳间隔（秒）。
///
/// 两个作用：① 穿透 nginx / CDN 等中间层的「空闲即断开」；
/// ② 让前端能区分「还在算」与「连接已死」——没有心跳，前端只能靠总超时，
/// 而 200 秒的流程里总超时设多少都不合适。
const HEARTBEAT_SECS: u64 = 15;

pub const EV_HELLO: &str = "hello";
pub const EV_STEP_START: &str = "step_start";
pub const EV_STEP_DELTA: &str = "step_delta";
pub const EV_STEP_RETRY: &str = "step_retry";
pub const EV_STEP_DONE: &str = "step_done";
pub const EV_STEP_FAIL: &str = "step_fail";
pub const EV_RED_FLAG: &str = "red_flag";
pub const EV_BLOCKED: &str = "blocked";
pub const EV_SKIPPED: &str = "skipped";
pub const EV_LOOP: &str = "loop";
pub const EV_CONFIDENCE: &str = "confidence";
pub const EV_SUMMARY: &str = "summary";
pub const EV_DONE: &str = "done";
pub const EV_ERROR: &str = "error";

/// 一条 SSE 事件
#[derive(Debug, Clone)]
pub struct StreamEvent {
    /// 事件名（`event:` 字段）
    pub event: &'static str,
    /// 事件负载（`data:` 字段，恒为单行 JSON）
    pub data: Value,
}

impl StreamEvent {
    pub fn new(event: &'static str, data: Value) -> Self {
        Self { event, data }
    }

    /// 编码成一帧 SSE。
    ///
    /// `data` 由 `serde_json` 序列化，正文里的换行会被转义成 `\n`，
    /// 故整帧恒为三行，不会与帧分隔符 `\n\n` 混淆。
    pub fn encode(&self) -> String {
        format!("event: {}\ndata: {}\n\n", self.event, self.data)
    }
}

impl Clone for StreamSink {
    fn clone(&self) -> Self {
        let tx = match self.tx.lock() {
            Ok(g) => g.as_ref().map(|t| t.clone()),
            Err(poisoned) => poisoned.into_inner().as_ref().map(|t| t.clone()),
        };
        Self {
            tx: Mutex::new(tx),
            connected: AtomicBool::new(self.connected.load(Ordering::Relaxed)),
        }
    }
}

/// 事件推送句柄：非流式调用时为空实现，编排器无需分支判断。
///
/// 用 `Mutex<Option<Sender>>` 而不是 `Option<Sender>`：
/// 断连检测要在 `&self` 下修改自身（置空通道），需要内部可变性。
/// 推送不在关键路径上（单步几十毫秒到几十秒），锁竞争可以忽略。
pub struct StreamSink {
    tx: Mutex<Option<mpsc::UnboundedSender<StreamEvent>>>,
    /// 是否**曾经**连接过（区分「从未连接」与「连接后断开」）
    connected: AtomicBool,
}

impl StreamSink {
    /// 非流式：所有 `emit` 静默丢弃（`/chat`、MCP、`llm_eval` 走这条）
    pub fn disabled() -> Self {
        Self {
            tx: Mutex::new(None),
            connected: AtomicBool::new(false),
        }
    }

    /// 流式：事件经 `tx` 推给 SSE 响应体
    pub fn new(tx: mpsc::UnboundedSender<StreamEvent>) -> Self {
        Self {
            tx: Mutex::new(Some(tx)),
            connected: AtomicBool::new(true),
        }
    }

    /// 是否真的在推流（编排器据此跳过纯展示用的计算）
    pub fn is_enabled(&self) -> bool {
        self.tx.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// 推送一条事件；客户端已断开时置空通道，后续 `emit` 全部丢弃。
    pub fn emit(&self, event: &'static str, data: Value) {
        let mut guard = match self.tx.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(tx) = guard.as_ref() else {
            return;
        };
        if tx.send(StreamEvent::new(event, data)).is_err() {
            tracing::debug!(event, "流式客户端已断开，停止推送后续事件");
            *guard = None;
        }
    }

    /// 客户端是否**曾经**连接但已断开。
    ///
    /// 与 `!is_enabled()` 的区别：`disabled()` 的 sink 从未连接过，
    /// 而这里表示连上又断了。编排器只对后者提前终止——前者的调用方（`/chat`）
    /// 仍然在等完整结果，中途停掉会返回一份残缺报告。
    pub fn is_closed(&self) -> bool {
        // 需要区分「从未连接」与「连接后断开」，故额外记一个标志位。
        self.connected.load(Ordering::Relaxed) && !self.is_enabled()
    }
}

/// 把事件接收端包成 SSE 响应。
///
/// 手写响应头而不用 `axum::response::Sse`：这里要能控制 `X-Accel-Buffering`
/// （关掉 nginx 缓冲，否则 SSE 会被攒成一次性返回，流式白做）。
pub fn sse_response(rx: mpsc::UnboundedReceiver<StreamEvent>) -> Response {
    let heartbeat = Duration::from_secs(HEARTBEAT_SECS);
    let stream = futures::stream::unfold(rx, move |mut rx| async move {
        match tokio::time::timeout(heartbeat, rx.recv()).await {
            // 超时未收到事件：发心跳保活，继续等
            Err(_) => Some((String::from(": ping\n\n"), rx)),
            Ok(Some(ev)) => Some((ev.encode(), rx)),
            // 发送端全部释放：流正常结束
            Ok(None) => None,
        }
    })
    .map(|s| Ok::<Bytes, std::io::Error>(Bytes::from(s)));

    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache, no-transform")
        .header(header::CONNECTION, "keep-alive")
        // nginx 的 `proxy_buffering` 默认开启，会把 SSE 攒满一个缓冲区再下发，
        // 流式因此退化成一次性返回。这个响应头让 nginx 对该响应单独关闭缓冲。
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "构造 SSE 响应失败");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        })
}

/// token 增量出口：把某个步骤的 LLM 输出逐片推给前端
///
/// 带上 `index` 与 `capability`，是因为采集期**四步并行**——
/// 四路 token 会交织着往外推，前端必须知道每一片属于哪一步才能归位，
/// 否则四段文字会被拼成一锅粥。
#[derive(Clone)]
pub struct DeltaSink {
    sink: StreamSink,
    index: usize,
    capability: Capability,
}

impl DeltaSink {
    pub fn new(sink: StreamSink, index: usize, capability: Capability) -> Self {
        Self {
            sink,
            index,
            capability,
        }
    }

    /// 推一片文本；无内容（纯工具调用片）时不发帧
    pub fn push(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.sink.emit(
            EV_STEP_DELTA,
            json!({
                "index": self.index,
                "capability": self.capability.slug(),
                "delta": text,
            }),
        );
    }

    /// 告知前端「本步正在重试」。
    ///
    /// 为什么非推不可（真机实测的教训）：
    /// ① 单次超时 120s × 最多 3 次尝试 = **6 分钟里客户端一个字都收不到**，
    ///    只有心跳——用户看到的是「还在算」，与卡死完全无法区分；
    /// ② 重试会把正文**从头再推一遍**，前端按 `index` 累加就拼出两份重复内容。
    ///    实测一次开方步 `duration_ms=361504`，全耗在这三次尝试上。
    ///
    /// 故这帧同时是一个**信号**：已推的 `step_delta` 作废，请丢弃后重新累积。
    pub fn emit_retry(&self, attempt: u32, max_retries: u32, error: &str) {
        self.sink.emit(
            EV_STEP_RETRY,
            json!({
                "index": self.index,
                "capability": self.capability.slug(),
                "attempt": attempt,
                "max_retries": max_retries,
                "error": error,
            }),
        );
    }

    /// 客户端是否已断开（编排器据此停止后续步骤）
    pub fn is_closed(&self) -> bool {
        self.sink.is_closed()
    }
}

/// 步骤计划的单项（`hello` 事件的 `plan` 元素）
pub fn plan_item(index: usize, cap: Capability, phase: &'static str) -> Value {
    json!({
        "index": index,
        "capability": cap.slug(),
        "zh": cap.zh(),
        "phase": phase,
    })
}
