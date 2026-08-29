//! 调用级埋点（observability）
//!
//! 每个 sub-agent 步骤共享一个 `StepMetrics` 累加器：LLM 调用的耗时、重试次数、
//! token 用量，以及工具调用名与错误都记入其中。`orchestrator` 在每步结束后
//! 把累加器快照成 `StepTrace`：既写入 tracing 日志，也随 `/chat` 响应返回，
//! 使「某一步慢/贵/失败」可被直接观测，而不必重放整次问诊。

use serde::Serialize;
use std::sync::{Arc, Mutex};

use crate::model::Capability;

/// 单次 LLM 调用的度量
#[derive(Debug, Clone, Default)]
pub struct LlmCallStat {
    /// 本次调用的墙钟耗时（含重试则为累计）
    pub duration_ms: u128,
    /// 实际发起的请求次数（1 表示一次成功）
    pub attempts: u32,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    /// 失败原因（成功时为 None）
    pub error: Option<String>,
}

/// 一个 sub-agent 步骤内的调用累加器
#[derive(Debug, Clone, Default, Serialize)]
pub struct StepMetrics {
    /// LLM 调用次数（多轮工具调用下 > 1）
    pub llm_calls: u32,
    /// LLM 请求总次数（含重试）
    pub llm_attempts: u32,
    /// LLM 请求累计耗时（毫秒）
    pub llm_duration_ms: u128,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    /// 实际调用过的工具名（按调用顺序，可重复）
    pub tool_calls: Vec<String>,
    /// 最近一次错误（LLM 或工具执行失败）
    pub last_error: Option<String>,
}

impl StepMetrics {
    /// 累加一次 LLM 调用：token 按轮次**求和**，错误只保留最近一次
    pub fn record_llm(&mut self, stat: &LlmCallStat) {
        self.llm_calls += 1;
        self.llm_attempts += stat.attempts.max(1);
        self.llm_duration_ms += stat.duration_ms;
        self.prompt_tokens = add_opt(self.prompt_tokens, stat.prompt_tokens);
        self.completion_tokens = add_opt(self.completion_tokens, stat.completion_tokens);
        self.total_tokens = add_opt(self.total_tokens, stat.total_tokens);
        if let Some(e) = &stat.error {
            self.last_error = Some(e.clone());
        }
    }

    /// 记录一次工具调用
    pub fn record_tool(&mut self, name: &str) {
        self.tool_calls.push(name.to_string());
    }

    /// 记录一次错误（工具执行失败等）
    pub fn record_error(&mut self, msg: String) {
        self.last_error = Some(msg);
    }
}

fn add_opt(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (Some(x), None) => Some(x),
        (None, y) => y,
    }
}

/// 步骤内共享的埋点句柄（跨多轮工具调用与重试累积）
pub type TraceHandle = Arc<Mutex<StepMetrics>>;

/// 新建埋点句柄
pub fn new_trace() -> TraceHandle {
    Arc::new(Mutex::new(StepMetrics::default()))
}

/// 在句柄上记录；`None` 表示不采集（单测直接调 LLM 辅助函数时用得上）
pub fn record<F: FnOnce(&mut StepMetrics)>(trace: Option<&TraceHandle>, f: F) {
    if let Some(t) = trace {
        // 埋点不在关键路径上：锁被 poison 时静默跳过，不影响问诊结果
        if let Ok(mut m) = t.lock() {
            f(&mut m);
        }
    }
}

/// 取当前快照（锁不可用时退化为默认值）
pub fn snapshot(trace: &TraceHandle) -> StepMetrics {
    trace
        .lock()
        .map(|m| m.clone())
        .unwrap_or_else(|e| e.into_inner().clone())
}

/// 一次 sub-agent 步骤的完整埋点（`orchestrator` 逐步汇总产出）
#[derive(Debug, Clone, Serialize)]
pub struct StepTrace {
    pub capability: Capability,
    /// 中文名（日志与前端展示直接可用）
    pub name: &'static str,
    /// 步骤总耗时（毫秒，含规则计算与 LLM 等待）
    pub duration_ms: u128,
    pub model: String,
    pub llm_calls: u32,
    pub llm_attempts: u32,
    pub llm_duration_ms: u128,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub tool_calls: Vec<String>,
    pub error: Option<String>,
}
