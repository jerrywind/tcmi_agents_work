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

/// 证候锁定的最低置信度（H4）
///
/// **低于此值不锁定**：把一个勉强凑出来的证候钉死给治疗期，开方步就会按
/// 一个可能是错的证去查方、开方，而报告里看不出这是猜的。
///
/// 为什么不干脆沿用 `MIN_CONFIDENCE`（0.2）：那是「进不进候选集」的门槛，
/// 宽松是为了让模型看到备选；而锁定是「把它当结论往下传」，必须更严。
/// 0.4 对应约 2 条主症证据（2 × 1.0 / 5.0）。
pub const LOCK_MIN_CONFIDENCE: f64 = 0.4;

/// 证候锁定结果（H4）
///
/// 供编排器判断「这次结论可不可信」，并把不确定的事实一路带到响应里。
#[derive(Debug, Clone)]
pub struct SyndromeLock {
    pub slug: String,
    pub name: String,
    pub confidence: f64,
    /// 是否已注入治疗期 payload。
    ///
    /// `false` 表示辨证出了主证但置信度不足，治疗期各步退回文本推断——
    /// 此时报告必须显式标注不确定，否则读者会把它当成确定结论。
    pub locked: bool,
}

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
    /// 结论可信度提示（H4 / H5）：`Some` 时置顶于正文，并随响应下发。
    ///
    /// 覆盖三种「报告看起来正常、其实不可信」的情形：
    /// ① 未匹配到明确证候；② 匹配到但置信度不足、未锁定；
    /// ③ 达到最大追问轮次被强制放行。
    /// 此前这三条全部静默——T7.9 已为「典籍不可见」做了显著标注，
    /// 而这几条是更严重的信号却没有痕迹。
    pub confidence_note: Option<String>,
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
    // RAG 可达性（T7.9）：决定开方步「有没有典籍出处可引」
    rag: &crate::rag_health::SharedRagStatus,
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

    let (collection, diagnosis_phase, post_phase) = split_phases(&order);

    // 安全门判定基于用户文本，与 SafetyAgent 共用同一函数，避免两处口径不一致
    let corpus: String = safety_corpus(messages);

    // RAG 可达性对**所有**阶段都有用，不只是开方：医案参考步同样依赖典籍，
    // 检索不到时会凭记忆复述出看似真实、实则编造的病案（T7.9）。
    let base_payload = with_rag_status(payload.clone(), rag);

    // ---- 安全门预检（T7.7）：合规红线，先于一切 ----
    //
    // 原先安全门排在治疗期（Phase C），而收敛判定在它**之前**：信息不足以辨证时
    // 流程会停下来追问（awaiting_input），安全门根本不执行。真实验证里
    // 「突然胸痛剧烈、出冷汗、呼吸困难、左臂发麻」这条典型心梗表现因此被漏检，
    // 患者反而被追问「舌苔什么颜色」——把安全放在信息完整之后，是危险的。
    //
    // 红旗判定是关键词匹配（纯函数、零延迟），故可以先进预检：
    // 命中就没必要再跑四诊与辨证，只让安全门产出可读警示后立即返回。
    let safety_enabled = order.contains(&Capability::Safety);
    let red_flag_prehit = safety_enabled && blocking_red_flag(res, &corpus).is_some();
    if red_flag_prehit {
        tracing::warn!("安全门预检命中：跳过四诊与辨证，直接产出拦截结论");
    }

    // ---- Phase A：采集（望闻问切并行）----
    //
    // 四诊之间互不依赖，串行跑等于把 4 次 LLM 往返的时间累加起来。
    // 并行后总耗时取决于最慢的一步。
    if !red_flag_prehit {
        let outcomes = run_parallel(
            registry,
            cfg,
            res,
            llm,
            skills,
            &collection,
            messages,
            &base_payload,
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
    }

    // ---- 安全门：采集一结束就过，不等辨证、不受收敛判定影响 ----
    //
    // 与 SafetyAgent 共用同一个 `blocking_red_flag` 判定（见 `absorb`）。
    // 预检命中时同样要跑：它不只是判定，还要产出患者读得懂的警示文案。
    if safety_enabled {
        if let Some(o) = run_step(
            registry,
            cfg,
            res,
            llm,
            skills,
            Capability::Safety,
            messages,
            &base_payload,
        )
        .await
        {
            absorb(
                vec![o],
                &mut steps,
                &mut failures,
                &mut traces,
                &mut structured,
                &mut blocked,
                res,
                &corpus,
            );
        }
    }

    // ---- Phase B：辨证（医案参考 → 辨证，串行：后者要用前者的参照）----
    // 已被安全门拦截时不再辨证：结论已定（转诊就医），再辨只是白跑一次 LLM。
    if blocked.is_none() {
        for cap in &diagnosis_phase {
            match run_step(
                registry,
                cfg,
                res,
                llm,
                skills,
                *cap,
                messages,
                &base_payload,
            )
            .await
            {
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
    }

    // ---- 收敛判定：信息不够就停下来问，而不是硬着头皮往下走 ----
    //
    // 调用方已在 `payload.syndrome` 里给定证候时跳过——那是「已知证候求方剂」
    // 的场景，再追问反而是打扰。
    let has_explicit_syndrome = payload.get("syndrome").and_then(|v| v.as_str()).is_some();
    if blocked.is_none() && !has_explicit_syndrome && !diagnosis_phase.is_empty() {
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

    // ---- 证候锁定（T7.1）：把辨证结论钉死，再交给治疗期各步 ----
    //
    // 此前 Phase C 直接透传调用方的原始 `payload`，于是立法/用药/开方各自
    // 调用 `infer_syndrome_slug` 从原始对话**重新猜一遍**证候（关键词计数，
    // 与辨证步的 `assess()` 不同算法、无矛盾证据、无阈值）。结果是同一份
    // 语料里辨证步判「脾胃湿热」、开方步按「肝胆湿热」开龙胆泻肝汤——
    // 人工验收的「方剂与主证不对口」「前后不一致」都源于此。
    //
    // 辨证一旦完成，主证就是后续步骤的共同前提：把它注入 payload，
    // 各步经 `resolve_syndrome` 读到的必然是同一结论。
    let (post_payload, syndrome_lock) = lock_syndrome(res, messages, &base_payload);

    // ---- Phase C：治疗（仅未被拦截且已收敛时执行）----
    if blocked.is_none() && !awaiting_input {
        for cap in &post_phase {
            match run_step(
                registry,
                cfg,
                res,
                llm,
                skills,
                *cap,
                messages,
                &post_payload,
            )
            .await
            {
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

    // 结论可信度提示（H4 / H5）：置顶于正文，且**先于**安全门拦截拼接，
    // 让更紧急的拦截信息留在最顶部。
    //
    // 它是「这份报告能信到什么程度」的唯一可见线索——此前未定证、
    // 低置信度、强制放行三种情形全部静默，读报告的人无从分辨。
    let confidence_note = build_confidence_note(syndrome_lock.as_ref(), loop_state.as_ref());
    if let Some(note) = &confidence_note {
        final_text = format!("## ⚠️ 结论可信度提示\n{note}\n\n{final_text}");
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
        confidence_note,
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

/// 安全门判定语料：**只取患者陈述**
///
/// 曾经把全部消息（含 assistant）拼在一起，于是从第二轮起，上一轮安全门
/// 自己输出的警示文案（「若出现胸痛、呼吸困难、咯血…请立即就医」）也被算进语料，
/// 预检**必然命中**——患者老老实实补答之后反而被拦截，永远拿不到方案。
/// 真实验证里第二轮就只剩 safety 一步、state 直接 completed。
///
/// 助手说的话是系统的推测与警示，**不构成「患者有此症状」的证据**；
/// 把它当证据，等于让系统在自己的回声里越听越像急症。
///
/// 代价：由助手追问、患者只答「有」的症状（如问「有没有胸痛」答「有」）不会被
/// 计入。急危重症通常由患者主动陈述，且漏判一侧还有 SafetyAgent 的 LLM 复核兜底；
/// 而误判一侧是「健康用户永远拿不到方案」，两害相权取后者。
pub fn safety_corpus(messages: &[Message]) -> String {
    messages
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 阶段编号：0 采集 / 1 辨证 / 2 治疗
fn phase_of(cap: Capability) -> u8 {
    if Capability::COLLECTION.contains(&cap) {
        0
    } else if matches!(cap, Capability::CaseReference | Capability::Differentiation) {
        1
    } else {
        2
    }
}

/// 把执行顺序切成三段：采集 / 辨证 / 治疗
///
/// **安全门不在任何一段里**（T7.7）：它有独立的阶段，先于辨证与收敛判定执行。
/// 曾经它归在治疗期，于是排在收敛判定之后——信息不足以辨证时流程会停下来
/// 追问，安全门**整个被跳过**，典型心梗表现因此漏检。
///
/// 抽成纯函数就是为了能直接断言这条不变量：安全门的位置是合规红线，
/// 不能只靠「读代码时记得」。
pub fn split_phases(order: &[Capability]) -> (Vec<Capability>, Vec<Capability>, Vec<Capability>) {
    let mut collection = Vec::new();
    let mut diagnosis = Vec::new();
    let mut post = Vec::new();
    for c in order {
        match phase_of(*c) {
            0 => collection.push(*c),
            1 => diagnosis.push(*c),
            _ => {
                if *c != Capability::Safety {
                    post.push(*c);
                }
            }
        }
    }
    (collection, diagnosis, post)
}

/// 把 RAG 可达性写入治疗期 payload（T7.9）
///
/// 开方步据此判断「能不能引用典籍出处」。RAG 不可用时模型照样会写
/// 「出自《xxx》」——真实验证里就把龙胆泻肝汤标成了出自《伤寒论》
/// （实出自《医方集解》）。把「本次没检索到典籍」这个事实传下去，
/// 模型才有可能不编书名，报告里也会留下一句可核对的说明。
///
/// 探测失败（`reachable = None`，例如还没探测过）按不可用处理：
/// 宁可少引一处出处，也不要让「未经核对」冒充「有典籍支撑」。
pub fn with_rag_status(
    payload: serde_json::Value,
    rag: &crate::rag_health::SharedRagStatus,
) -> serde_json::Value {
    let reachable = match rag.read() {
        Ok(g) => g.reachable,
        Err(poisoned) => poisoned.into_inner().reachable,
    };
    let Some(obj) = payload.as_object() else {
        return payload;
    };
    let mut out = obj.clone();
    out.insert("rag_available".into(), json!(reachable.unwrap_or(false)));
    serde_json::Value::Object(out)
}

/// 把辨证结论注入治疗期各步的 payload（T7.1 证候锁定）
///
/// 写入字段：
/// - `syndrome`：主证 slug（各步经 `resolve_syndrome` 读取）
/// - `syndrome_name` / `syndrome_confidence`：主证中文名与置信度
/// - `concurrent_syndromes`：兼证 slug 列表（无兼证时不写）
///
/// 调用方已在 `payload.syndrome` 显式给定证候时**原样保留**——
/// 那是「已知证候求方剂」的场景，覆盖它等于无视调用方的判断。
///
/// 证据不足（`assess` 未产出主证）时也不写：此时宁可让各步退回文本推断，
/// 也好过把一个 `None` 当结论传递下去。
///
/// H4：置信度低于 [`LOCK_MIN_CONFIDENCE`] 时同样不写。
/// 此前只要 `assess()` 产出主证就锁定，而 `assess()` 是在有限的证候库里
/// 必选其一，`MIN_CONFIDENCE` 又只有 0.2——命中一条「乏力」就能攒出 0.30
/// 并被当成结论传给开方步。**不锁定 ≠ 不告知**：返回值仍带回主证信息，
/// 由编排器把它变成报告顶部那句「证据不足」。
///
/// 写成 `pub` 是为了可测：这是纯函数（同输入必同输出），
/// 不暴露就只能靠连真实 LLM 的端到端去验证这条最容易出错的链路。
pub fn lock_syndrome(
    res: &ResourceBundle,
    messages: &[Message],
    payload: &serde_json::Value,
) -> (serde_json::Value, Option<SyndromeLock>) {
    let Some(obj) = payload.as_object() else {
        return (payload.clone(), None);
    };
    // 支持中文名与 slug 两种写法（与 `resolve_syndrome` 同一套归一化）
    let explicit_slug = obj
        .get("syndrome")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .and_then(|raw| crate::agents::normalize_syndrome_slug(res, raw));
    if let Some(slug) = explicit_slug {
        let name = res
            .syndrome(&slug)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        return (
            payload.clone(),
            Some(SyndromeLock {
                slug,
                name,
                // 调用方显式给定的证候是「已知证候求方剂」的前提，不参与置信度门槛
                confidence: 1.0,
                locked: true,
            }),
        );
    }
    if obj
        .get("syndrome")
        .and_then(|v| v.as_str())
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
    {
        // 显式给了但不在证候库里：原样透传（各步 `resolve_syndrome` 会 WARN
        // 后回退文本推断），不冒充已锁定。
        return (payload.clone(), None);
    }

    let result = crate::agents::differentiation::assess(res, messages);
    let Some(primary) = result.primary else {
        tracing::info!("证候锁定：未匹配到明确证候，治疗期各步退回文本推断");
        // 显式写下「未匹配」：治疗期各步据此在 system 里标注不确定（H6）。
        // 不能只靠「payload 里没有 syndrome」来推断——单步调用
        // （`POST /agents`）本来就不带证候，那是调用方没给，不是没辨出来。
        let mut out = obj.clone();
        out.insert("syndrome_matched".into(), json!(false));
        return (serde_json::Value::Object(out), None);
    };

    let lock = SyndromeLock {
        slug: primary.slug.clone(),
        name: primary.name.clone(),
        confidence: primary.confidence,
        locked: primary.confidence >= LOCK_MIN_CONFIDENCE,
    };

    if !lock.locked {
        tracing::warn!(
            syndrome = %primary.slug,
            confidence = primary.confidence,
            threshold = LOCK_MIN_CONFIDENCE,
            "证候锁定：置信度不足，不注入治疗期（报告将标注证据不足）"
        );
        // 仍把主证与置信度写进去，供治疗期各步显式标注不确定性（H6）
        let mut out = obj.clone();
        out.insert("syndrome_name".into(), json!(primary.name));
        out.insert("syndrome_confidence".into(), json!(primary.confidence));
        out.insert("syndrome_matched".into(), json!(true));
        out.insert("syndrome_locked".into(), json!(false));
        return (serde_json::Value::Object(out), Some(lock));
    }

    let mut out = obj.clone();
    out.insert("syndrome".into(), json!(primary.slug));
    out.insert("syndrome_name".into(), json!(primary.name));
    out.insert("syndrome_confidence".into(), json!(primary.confidence));
    out.insert("syndrome_matched".into(), json!(true));
    out.insert("syndrome_locked".into(), json!(true));
    if !result.concurrent.is_empty() {
        let slugs: Vec<String> = result.concurrent.iter().map(|c| c.slug.clone()).collect();
        out.insert("concurrent_syndromes".into(), json!(slugs));
    }
    tracing::info!(
        syndrome = %primary.slug,
        confidence = primary.confidence,
        concurrent = result.concurrent.len(),
        "证候锁定：辨证主证已注入治疗期各步"
    );
    (serde_json::Value::Object(out), Some(lock))
}

/// 结论可信度提示（H4 / H5）
///
/// 三条规则各自对应一种「报告看起来正常、其实不可信」的情形，
/// 命中多条时合并输出——它们常常同时成立（信息不足 → 置信度低 → 又被强制放行）。
pub fn build_confidence_note(
    lock: Option<&SyndromeLock>,
    conv: Option<&Convergence>,
) -> Option<String> {
    let mut notes: Vec<String> = Vec::new();

    match lock {
        None => notes.push(
            "本次**未匹配到明确证候**：四诊信息未满足任一候选证候的主症必备条件，\
             下方内容为一般性参考，不构成辨证结论，请线下就诊明确。"
                .to_string(),
        ),
        Some(l) if !l.locked => notes.push(format!(
            "本次辨证**置信度偏低**：主证「{}」置信度仅 {:.2}，未达锁定门槛 {:.2}，\
             治疗建议按该证候给出但未经核实，请线下就诊确认后再行用药。",
            l.name, l.confidence, LOCK_MIN_CONFIDENCE
        )),
        _ => {}
    }

    if let Some(c) = conv {
        if c.forced {
            notes.push(format!(
                "已达最大追问轮次（{} 轮），信息仍不完整（覆盖率 {:.0}%）即给出结论：\
                 以下建议仅供参考，建议线下就诊补足四诊信息。",
                c.round,
                c.coverage * 100.0
            ));
        }
    }

    if notes.is_empty() {
        None
    } else {
        Some(notes.join("\n"))
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
        // 结论可信度（H4 / H5）：`low_confidence` 供调用方做门禁式判断，
        // `confidence_note` 是可直接展示给用户的中文说明。
        // 此前「未定证 / 置信度不足 / 强制放行」三种情形在响应里毫无痕迹。
        "low_confidence": d.confidence_note.is_some(),
        "confidence_note": d.confidence_note,
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
