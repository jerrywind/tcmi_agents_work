//! 诊断流程引擎（orchestrator）
//!
//! ## 两阶段反馈式辨证
//!
//! ```text
//! Phase A 采集（望闻问切，并行）→ Phase B 辨证（医案参考 + 辨证）→ 收敛？
//!                                        ↑                              │
//!                                        └── 不收敛：返回追问，等用户补充 ─┘
//!                                                                        │ 收敛
//!                                                                        ↓
//!                                              Phase C 安全门 → 立法 → 用药 → 开方
//! ```
//!
//! 为什么要 loop：患者常常只说「咳嗽两天」，此时硬辨证置信度只有 0.2，
//! 却照样往下走到开方——开出来的方自然不可靠。真实中医靠追问补齐信息，
//! 这里用 [`crate::agents::convergence`] 把收敛判定做成确定性规则。
//!
//! 单轮内**不会**让模型「再想想」：真实信息只能来自用户，空转只会产生幻觉。
//! 不收敛就返回 `awaiting_input` + 追问，等下一轮携带新信息进来。
//!
//! 四个横切行为：
//! - **部分失败降级**：某步失败只记录，不中断，已完成步骤照常返回；
//! - **红旗中断**（T3.3）：安全门命中 high/critical 时终止后续步骤，
//!   并产出结构化 `blocked` 标记，调用方无需解析正文文本即可判断；
//! - **动态科室注入**：辨证后把相关科室写入共享状态，`tcm-rag` 据此
//!   收窄检索范围（辨证出儿科，开方就只看儿科方书）；
//! - **逐步埋点**（T3.1）：每步记录耗时 / 模型 / token / 工具调用 / 错误。

use crate::agents::convergence::{evaluate, Convergence, LoopConfig};
use crate::agents::{blocking_red_flag, Registry};
use crate::config::HarnessConfig;
use crate::model::{Capability, Message};
use crate::resources::ResourceBundle;
use crate::skills::SharedDepartments;
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
#[derive(Default)]
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
    /// 反馈式辨证 loop 的状态；`None` 表示未启用（如调用方已给定证候）
    pub loop_state: Option<Convergence>,
    /// 是否**停下来等用户补充信息**（未收敛且未强制放行时为 true）。
    ///
    /// 为 true 时后续步骤未执行，响应里带 `pending_questions`；
    /// 调用方应把用户回答追加进 `messages` 并再次请求（轮次 `payload.round` +1）。
    pub awaiting_input: bool,
}

/// 解析执行顺序：**安全门不可被配置移除**（T5.4 合规）
///
/// `routing.yaml` 的 `active` 允许增删步骤（如跳过切诊），但安全门是**合规底线**：
/// 若允许把它从流程里删掉，红旗症状就会绕过拦截直接走到治疗建议，
/// 这是本系统最不能出错的一条路径。故此处强制补齐：
/// - `active` 里已有 safety：原样使用；
/// - 缺失：插到治疗步之前（无治疗步则追加到末尾），并告警。
pub fn resolve_order(res: &ResourceBundle) -> Vec<Capability> {
    // 命名档位优先：`profiles` 让「兼容 / 标准 / 完整」三套流程可一键切换，
    // 不必每次手动增删 active 列表。
    let from_profile = res
        .routing
        .active_profile
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .and_then(|p| res.routing.profiles.get(p.trim()))
        .map(|list| {
            list.iter()
                .filter_map(|s| Capability::from_slug(s))
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty());

    let order: Vec<Capability> = match from_profile {
        Some(v) => v,
        None if !res.routing.active.is_empty() => res
            .routing
            .active
            .iter()
            .filter_map(|s| Capability::from_slug(s))
            .collect(),
        // 未配置任何流程时退回「标准档」：兼容档会让新增的立法/用药/开方
        // 永远不被用上，标准档才是本系统的设计形态。
        None => Capability::ALL
            .iter()
            .copied()
            .filter(|c| {
                !matches!(
                    c,
                    Capability::Care | Capability::Acupuncture | Capability::Treatment
                )
            })
            .collect(),
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
/// `departments` 是编排器与 `tcm-rag` 之间的共享「当前科室」。
/// 返回每一步的输出，便于前端分步展示。
///
/// 流程分三段（见模块文档）：采集（并行）→ 辨证 → 收敛判定 → 安全门与治疗。
#[allow(clippy::too_many_arguments)]
pub async fn run_diagnosis(
    registry: &Registry,
    cfg: &HarnessConfig,
    res: &ResourceBundle,
    llm: &reqwest::Client,
    skills: &SkillRegistry,
    departments: &SharedDepartments,
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
    let mut loop_state = None;
    let mut awaiting_input = false;

    /// 阶段：0 采集 / 1 辨证 / 2 安全门与治疗
    fn phase_of(cap: Capability) -> u8 {
        if Capability::COLLECTION.contains(&cap) {
            0
        } else if matches!(cap, Capability::CaseReference | Capability::Differentiation) {
            1
        } else {
            2
        }
    }

    let collection: Vec<Capability> = order
        .iter()
        .copied()
        .filter(|c| phase_of(*c) == 0)
        .collect();
    let diagnosis_phase: Vec<Capability> = order
        .iter()
        .copied()
        .filter(|c| phase_of(*c) == 1)
        .collect();
    let post_phase: Vec<Capability> = order
        .iter()
        .copied()
        .filter(|c| phase_of(*c) == 2)
        .collect();

    // 安全门判定基于用户文本，与 SafetyAgent 共用同一函数，避免两处口径不一致
    let corpus: String = messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");

    // ---- Phase A：采集（望闻问切并行）----
    //
    // 四诊之间互不依赖，串行跑等于把 4 次 LLM 往返的时间累加起来。
    // 并行后总耗时取决于最慢的一步。
    let outcomes = run_parallel(
        registry,
        cfg,
        res,
        llm,
        skills,
        &collection,
        messages,
        payload,
    )
    .await;
    absorb(
        outcomes,
        &mut steps,
        &mut failures,
        &mut traces,
        &mut structured,
        &mut blocked,
        res,
        &corpus,
    );

    // ---- Phase B：辨证（医案参考 → 辨证，串行：后者要用前者的参照）----
    for cap in &diagnosis_phase {
        match run_step(registry, cfg, res, llm, skills, *cap, messages, payload).await {
            Some(o) => absorb(
                vec![o],
                &mut steps,
                &mut failures,
                &mut traces,
                &mut structured,
                &mut blocked,
                res,
                &corpus,
            ),
            None => continue,
        }
    }

    // ---- 收敛判定：信息不够就停下来问，而不是硬着头皮往下走 ----
    //
    // 调用方已在 `payload.syndrome` 里给定证候时跳过——那是「已知证候求方剂」
    // 的场景，再追问反而是打扰。
    let has_explicit_syndrome = payload.get("syndrome").and_then(|v| v.as_str()).is_some();
    if !has_explicit_syndrome && !diagnosis_phase.is_empty() {
        let round = payload
            .get("round")
            .and_then(|v| v.as_u64())
            .map(|n| n.clamp(1, 255) as u8)
            .unwrap_or(1);
        let conv = evaluate(res, messages, &LoopConfig::default(), round);
        tracing::info!(
            round,
            converged = conv.converged,
            forced = conv.forced,
            confidence = conv.confidence,
            margin = conv.margin,
            coverage = conv.coverage,
            pending = conv.pending_questions.len(),
            "辨证收敛判定"
        );
        // 动态科室注入：辨证出儿科，后面开方就只看儿科方书
        inject_departments(res, &conv.primary_slug, departments);

        if !conv.converged {
            awaiting_input = true;
            loop_state = Some(conv);
        } else {
            loop_state = Some(conv);
        }
    }

    // ---- Phase C：安全门与治疗（仅在收敛后执行）----
    if !awaiting_input {
        for cap in &post_phase {
            match run_step(registry, cfg, res, llm, skills, *cap, messages, payload).await {
                Some(o) => absorb(
                    vec![o],
                    &mut steps,
                    &mut failures,
                    &mut traces,
                    &mut structured,
                    &mut blocked,
                    res,
                    &corpus,
                ),
                None => continue,
            }
            if blocked.is_some() {
                break;
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
        loop_state,
        awaiting_input,
    })
}

/// 单个步骤的执行结果（成功与失败同构，便于并行后统一归并）
struct StepOutcome {
    cap: Capability,
    result: Result<String, anyhow::Error>,
    trace: StepTrace,
    structured: Option<serde_json::Value>,
}

/// 执行一步；agent 未注册时返回 `None`（配置里写了但没实现，跳过即可）
#[allow(clippy::too_many_arguments)]
async fn run_step(
    registry: &Registry,
    cfg: &HarnessConfig,
    res: &ResourceBundle,
    llm: &reqwest::Client,
    skills: &SkillRegistry,
    cap: Capability,
    messages: &[Message],
    payload: &serde_json::Value,
) -> Option<StepOutcome> {
    let agent = registry.get(cap)?;
    let ctx = crate::agents::AgentContext::new(
        std::sync::Arc::new(cfg.clone()),
        std::sync::Arc::new(res.clone()),
        llm.clone(),
        std::sync::Arc::new(skills.clone()),
    );
    let started = Instant::now();
    // 单步失败只记录、不中断：已完成步骤的成本不应被后续失败浪费掉
    let result = agent.run(&ctx, messages, payload).await;
    let elapsed = started.elapsed().as_millis();

    match result {
        Ok(out) => {
            let trace = finish_trace(cap, &ctx, cfg, elapsed, None);
            tracing::info!(
                capability = ?cap,
                duration_ms = trace.duration_ms,
                llm_calls = trace.llm_calls,
                tool_calls = ?trace.tool_calls,
                "sub-agent 执行完成"
            );
            let structured = agent.structured(&ctx, messages);
            Some(StepOutcome {
                cap,
                result: Ok(out),
                trace,
                structured,
            })
        }
        Err(e) => {
            let msg = e.to_string();
            tracing::warn!(
                capability = ?cap,
                duration_ms = elapsed as u64,
                error = %msg,
                "sub-agent 执行失败，跳过该步骤"
            );
            Some(StepOutcome {
                cap,
                result: Err(e),
                trace: finish_trace(cap, &ctx, cfg, elapsed, Some(msg)),
                structured: None,
            })
        }
    }
}

/// 并行执行一组步骤（采集期用）
///
/// 四诊之间互不依赖，串行跑等于把 4 次 LLM 往返的时间累加。
#[allow(clippy::too_many_arguments)]
async fn run_parallel(
    registry: &Registry,
    cfg: &HarnessConfig,
    res: &ResourceBundle,
    llm: &reqwest::Client,
    skills: &SkillRegistry,
    caps: &[Capability],
    messages: &[Message],
    payload: &serde_json::Value,
) -> Vec<StepOutcome> {
    let futs: Vec<_> = caps
        .iter()
        .map(|cap| run_step(registry, cfg, res, llm, skills, *cap, messages, payload))
        .collect();
    // 结果按输入顺序返回（join_all 保序），故埋点顺序稳定
    futures::future::join_all(futs)
        .await
        .into_iter()
        .flatten()
        .collect()
}

/// 把一批步骤结果并入累加器
#[allow(clippy::too_many_arguments)]
fn absorb(
    outcomes: Vec<StepOutcome>,
    steps: &mut Vec<(Capability, String)>,
    failures: &mut Vec<(Capability, String)>,
    traces: &mut Vec<StepTrace>,
    structured: &mut Vec<(Capability, serde_json::Value)>,
    blocked: &mut Option<Blocked>,
    res: &ResourceBundle,
    corpus: &str,
) {
    for o in outcomes {
        traces.push(o.trace);
        match o.result {
            Ok(out) => {
                steps.push((o.cap, out));
                if let Some(v) = o.structured {
                    structured.push((o.cap, v));
                }
                // 安全门拦截：命中高危红色警戒则终止后续步骤（不再给治疗建议）
                if o.cap == Capability::Safety && blocked.is_none() {
                    if let Some(rf) = blocking_red_flag(res, corpus) {
                        tracing::warn!(
                            slug = %rf.slug,
                            severity = %rf.severity,
                            "安全门拦截：命中高危红色警戒，终止后续步骤"
                        );
                        *blocked = Some(Blocked {
                            by: o.cap,
                            slug: rf.slug.clone(),
                            label: rf.label.clone(),
                            severity: rf.severity.clone(),
                            advice: rf.advice.clone(),
                        });
                    }
                }
            }
            Err(e) => failures.push((o.cap, e.to_string())),
        }
    }
}

/// 把主证对应的科室写入共享状态，供 `tcm-rag` 动态收窄检索范围
fn inject_departments(res: &ResourceBundle, syndrome_slug: &str, departments: &SharedDepartments) {
    if syndrome_slug.is_empty() {
        return;
    }
    let Some(s) = res.syndrome(syndrome_slug) else {
        return;
    };
    if s.departments.is_empty() {
        return;
    }
    // 锁中毒时只告警：科室只是检索的增强条件，不该让整个问诊失败
    match departments.write() {
        Ok(mut guard) => {
            tracing::debug!(syndrome = %syndrome_slug, depts = ?s.departments,
                            "已注入动态科室");
            *guard = s.departments.clone();
        }
        Err(e) => tracing::warn!("动态科室注入失败（锁中毒）：{e}"),
    }
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
        // 反馈式辨证：`awaiting_input` 为 true 时流程停在了辨证之后，
        // 需把 `loop.pending_questions` 的答案追加进 messages 再请求一次
        // （并把 `payload.round` +1）。
        "status": if d.awaiting_input { "awaiting_input" } else { "completed" },
        "loop": d.loop_state.as_ref().map(|c| json!({
            "round": c.round,
            "converged": c.converged,
            "forced": c.forced,
            "confidence": c.confidence,
            "margin": c.margin,
            "coverage": c.coverage,
            "primary": c.primary_slug,
            "pending_questions": c.pending_questions,
        })),
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
