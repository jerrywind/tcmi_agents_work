//! 诊断流程引擎（orchestrator）
//!
//! 按 `resources/routing.yaml` 的激活顺序，依次调用 sub-agent，
//! 四诊收集证据 -> 辨证 -> 安全门 -> 治疗。
//!
//! 三个横切行为：
//! - **部分失败降级**：某步失败只记录，不中断，已完成步骤照常返回；
//! - **红旗中断**（T3.3）：安全门命中 high/critical 时终止后续步骤，
//!   并产出结构化 `blocked` 标记，调用方无需解析正文文本即可判断；
//! - **逐步埋点**（T3.1）：每步记录耗时 / 模型 / token / 工具调用 / 错误。

use crate::agents::{blocking_red_flag, Registry};
use crate::config::HarnessConfig;
use crate::model::{Capability, Message};
use crate::resources::ResourceBundle;
use crate::skills::SkillRegistry;
use crate::trace::{snapshot, StepTrace};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Instant;

/// 免责声明（T5.4 合规）
///
/// 必须由**服务端**随每份结果下发，而不只是前端写死一段话：
/// 任何接入方（含 MCP 客户端与第三方页面）拿到结论时都该同时拿到免责声明，
/// 否则「AI 健康建议」极易被误当作诊断。前端应优先展示本字段。
pub const DISCLAIMER: &str = "本内容由 AI 生成，仅供健康参考，不构成医疗诊断或处方建议。\
如有不适或出现胸痛、咯血、高热不退等警示症状，请及时线下就医。";

/// 安全门拦截信息（T3.3）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blocked {
    /// 由哪一步拦截（当前恒为 safety）
    pub by: Capability,
    /// 命中的红色警戒 slug / 中文名 / 级别 / 建议
    pub slug: String,
    pub label: String,
    pub severity: String,
    pub advice: String,
}

/// 单次诊断运行结果
pub struct Diagnosis {
    pub steps: Vec<(Capability, String)>,
    pub final_text: String,
    /// 执行失败的步骤及其错误原因。
    ///
    /// **部分失败降级**：某一步失败不再让整个 `/chat` 挂掉——
    /// 已完成的步骤照常返回，失败步骤记入这里，由调用方决定是否提示用户。
    /// （旧行为是用 `?` 直接向上传播，导致前几步的 LLM 开销全部白费。）
    pub failures: Vec<(Capability, String)>,
    /// 因安全门拦截而**未执行**的步骤（非失败，是主动跳过）
    pub skipped: Vec<(Capability, String)>,
    /// 安全门拦截信息；None 表示未拦截
    pub blocked: Option<Blocked>,
    /// 每步埋点（成功与失败各一条）
    pub trace: Vec<StepTrace>,
    /// 各步骤的结构化输出（T4.1）：按执行顺序，目前仅辨证步产出
    pub structured: Vec<(Capability, serde_json::Value)>,
}

/// 解析执行顺序：**安全门不可被配置移除**（T5.4 合规）
///
/// `routing.yaml` 的 `active` 允许增删步骤（如跳过切诊），但安全门是**合规底线**：
/// 若允许把它从流程里删掉，红旗症状就会绕过拦截直接走到治疗建议，
/// 这是本系统最不能出错的一条路径。故此处强制补齐：
/// - `active` 里已有 safety：原样使用；
/// - 缺失：插到治疗步之前（无治疗步则追加到末尾），并告警。
pub fn resolve_order(res: &ResourceBundle) -> Vec<Capability> {
    let order: Vec<Capability> = if res.routing.active.is_empty() {
        Capability::ALL.to_vec()
    } else {
        res.routing
            .active
            .iter()
            .filter_map(|s| Capability::from_slug(s))
            .collect()
    };

    if order.contains(&Capability::Safety) {
        return order;
    }

    let mut order = order;
    let pos = order
        .iter()
        .position(|c| *c == Capability::Treatment)
        .unwrap_or(order.len());
    order.insert(pos, Capability::Safety);
    tracing::warn!("routing.yaml 的 active 未包含 safety，已强制插入安全门（红旗路径不可移除）");
    order
}

/// 执行一次完整诊断流程。
///
/// `messages` 为截至当前的对话；`registry` 提供各 agent 实现，`skills` 提供工具。
/// 返回每一步的输出，便于前端分步展示。
pub async fn run_diagnosis(
    registry: &Registry,
    cfg: &HarnessConfig,
    res: &ResourceBundle,
    llm: &reqwest::Client,
    skills: &SkillRegistry,
    messages: &[Message],
    payload: &serde_json::Value,
) -> Result<Diagnosis> {
    let order = resolve_order(res);

    let mut steps = Vec::new();
    let mut failures = Vec::new();
    let mut skipped = Vec::new();
    let mut traces = Vec::new();
    let mut structured = Vec::new();
    let mut blocked = None;

    // 安全门判定基于用户文本，与 SafetyAgent 共用同一函数，避免两处口径不一致
    let corpus: String = messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");

    for cap in order {
        let Some(agent) = registry.get(cap) else {
            continue;
        };
        let ctx = crate::agents::AgentContext::new(
            std::sync::Arc::new(cfg.clone()),
            std::sync::Arc::new(res.clone()),
            llm.clone(),
            std::sync::Arc::new(skills.clone()),
        );

        let started = Instant::now();
        // 单步失败只记录、不中断：已完成步骤的成本不应被后续失败浪费掉
        let outcome = agent.run(&ctx, messages, payload).await;
        let elapsed = started.elapsed().as_millis();

        match outcome {
            Ok(out) => {
                let trace = finish_trace(cap, &ctx, cfg, elapsed, None);
                tracing::info!(
                    capability = ?cap,
                    duration_ms = trace.duration_ms,
                    llm_calls = trace.llm_calls,
                    llm_attempts = trace.llm_attempts,
                    tool_calls = ?trace.tool_calls,
                    "sub-agent 执行完成"
                );
                traces.push(trace);
                steps.push((cap, out));
                // 结构化输出：与正文同源、同一份输入，先后计算不影响结果
                if let Some(v) = agent.structured(&ctx, messages) {
                    structured.push((cap, v));
                }

                // 安全门拦截：命中高危红色警戒则终止后续步骤（不再给治疗建议）
                if cap == Capability::Safety {
                    if let Some(rf) = blocking_red_flag(res, &corpus) {
                        tracing::warn!(
                            slug = %rf.slug,
                            severity = %rf.severity,
                            "安全门拦截：命中高危红色警戒，终止后续步骤"
                        );
                        blocked = Some(Blocked {
                            by: cap,
                            slug: rf.slug.clone(),
                            label: rf.label.clone(),
                            severity: rf.severity.clone(),
                            advice: rf.advice.clone(),
                        });
                        break;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    capability = ?cap,
                    duration_ms = elapsed as u64,
                    error = %e,
                    "sub-agent 执行失败，跳过该步骤"
                );
                traces.push(finish_trace(cap, &ctx, cfg, elapsed, Some(e.to_string())));
                failures.push((cap, e.to_string()));
            }
        }
    }

    // 被拦截时，把未执行的步骤显式列出：调用方要能区分「失败」与「主动跳过」
    if let Some(b) = &blocked {
        let done: Vec<Capability> = steps.iter().map(|(c, _)| *c).collect();
        for cap in Capability::ALL {
            if done.contains(&cap) {
                continue;
            }
            skipped.push((
                cap,
                format!("安全门拦截（{}·{}），未执行", b.label, b.severity),
            ));
        }
    }

    let mut final_text = steps
        .iter()
        .map(|(c, t)| format!("## {}\n{t}", c.zh()))
        .collect::<Vec<_>>()
        .join("\n\n");

    // 失败步骤也在汇总里留痕，避免用户以为「没做这一步」是模型遗漏
    if !failures.is_empty() {
        let notes = failures
            .iter()
            .map(|(c, e)| format!("## {}\n（该步骤执行失败：{}）", c.zh(), e))
            .collect::<Vec<_>>()
            .join("\n\n");
        final_text = format!("{final_text}\n\n{notes}");
    }

    // 拦截提示置顶于正文：这是整次问诊里最该被看到的信息
    if let Some(b) = &blocked {
        final_text = format!(
            "## 安全门拦截\n命中{}：{}\n\n{}\n\n{}",
            b.label, b.advice, "已停止后续治疗建议，请及时就医。", final_text
        );
    }

    Ok(Diagnosis {
        steps,
        final_text,
        failures,
        skipped,
        blocked,
        trace: traces,
        structured,
    })
}

/// 单步调用（前端按 capability 直接请求某个 agent 时用）
///
/// 参数与 `run_diagnosis` 保持一致（注册表 / 配置 / 资源 / HTTP 客户端 / 技能），
/// 便于两个入口共用调用方；故豁免 `too_many_arguments`。
#[allow(clippy::too_many_arguments)]
pub async fn run_single(
    registry: &Registry,
    cfg: &HarnessConfig,
    res: &ResourceBundle,
    llm: &reqwest::Client,
    skills: &SkillRegistry,
    cap: Capability,
    messages: &[Message],
    payload: &serde_json::Value,
) -> Result<(Capability, String, StepTrace, Option<serde_json::Value>)> {
    if let Some(agent) = registry.get(cap) {
        let ctx = crate::agents::AgentContext::new(
            std::sync::Arc::new(cfg.clone()),
            std::sync::Arc::new(res.clone()),
            llm.clone(),
            std::sync::Arc::new(skills.clone()),
        );
        let started = Instant::now();
        let out = agent.run(&ctx, messages, payload).await?;
        let elapsed = started.elapsed().as_millis();
        let structured = agent.structured(&ctx, messages);
        Ok((
            cap,
            out,
            finish_trace(cap, &ctx, cfg, elapsed, None),
            structured,
        ))
    } else {
        anyhow::bail!("未注册的 capability: {:?}", cap)
    }
}

/// 把累加器快照成一条步骤埋点
fn finish_trace(
    cap: Capability,
    ctx: &crate::agents::AgentContext,
    cfg: &HarnessConfig,
    elapsed: u128,
    error: Option<String>,
) -> StepTrace {
    let m = snapshot(&ctx.trace);
    StepTrace {
        capability: cap,
        name: cap.zh(),
        duration_ms: elapsed,
        model: cfg.model.clone(),
        llm_calls: m.llm_calls,
        llm_attempts: m.llm_attempts,
        llm_duration_ms: m.llm_duration_ms,
        prompt_tokens: m.prompt_tokens,
        completion_tokens: m.completion_tokens,
        total_tokens: m.total_tokens,
        tool_calls: m.tool_calls,
        error: error.or(m.last_error),
    }
}

/// 构造标准化的 HTTP 响应 payload
///
/// 字段说明：
/// - `steps`：成功完成的步骤
/// - `summary`：全部步骤拼成的 Markdown
/// - `failures`：失败的步骤（含错误原因）
/// - `partial`：是否存在失败步骤（调用方据此提示「结果不完整」）
/// - `blocked` / `blocked_by` / `block_reason`：安全门拦截标记（T3.3）
/// - `skipped`：因拦截未执行的步骤
/// - `trace`：每步埋点（耗时 / token / 模型 / 工具 / 错误，T3.1）
/// - `structured`：各步骤的结构化输出，按 capability 键（T4.1，目前只有 `differentiation`）
pub fn diagnosis_payload(d: &Diagnosis) -> serde_json::Value {
    let steps: Vec<serde_json::Value> = d
        .steps
        .iter()
        .map(|(c, t)| json!({"capability": c, "text": t}))
        .collect();
    let failures: Vec<serde_json::Value> = d
        .failures
        .iter()
        .map(|(c, e)| json!({"capability": c, "error": e}))
        .collect();
    let skipped: Vec<serde_json::Value> = d
        .skipped
        .iter()
        .map(|(c, r)| json!({"capability": c, "reason": r}))
        .collect();
    // 以 capability slug 为键：调用方按 `structured.differentiation` 取用，
    // 后续步骤若要产出结构化结果，直接加键即可，不影响既有字段。
    let structured: serde_json::Map<String, serde_json::Value> = d
        .structured
        .iter()
        .filter_map(|(c, v)| {
            serde_json::to_value(c)
                .ok()
                .and_then(|k| k.as_str().map(|s| (s.to_string(), v.clone())))
        })
        .collect();

    json!({
        "steps": steps,
        "summary": d.final_text,
        // 合规（T5.4）：免责声明随每份结果下发，不依赖调用方自带
        "disclaimer": DISCLAIMER,
        "failures": failures,
        "partial": !d.failures.is_empty(),
        "blocked": d.blocked.is_some(),
        "blocked_by": d.blocked.as_ref().map(|b| b.by),
        "block_reason": d.blocked.as_ref().map(|b| format!(
            "{}·{}：{}", b.label, b.severity, b.advice
        )),
        "skipped": skipped,
        "trace": d.trace,
        "structured": structured,
    })
}
