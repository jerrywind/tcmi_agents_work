//! 技能判定引擎：在技能「生效」前做前置条件验证。
//!
//! 设计遵循单一职责 / 高内聚低耦合，各组件各管一摊、通过接口协作：
//!
//! | 组件 | 职责 |
//! |---|---|
//! | `Clock` | 时间来源抽象（真实 `SystemClock` / 测试 `MockClock`） |
//! | `CooldownTracker` | 仅负责技能冷却计时与「原子 arm」（检查并落点） |
//! | `ResourceLedger` | 仅负责资源余额的查询与原子扣减（CAS） |
//! | `StateProvider` | 仅负责提供「目标当前状态」 |
//! | `SkillRule` | 纯数据，描述一个技能的全部前置条件 |
//! | `JudgeError` | 判定失败的明确、可区分原因（异常处理） |
//! | `JudgeEvent` | 状态机事件（预检 / 各条件通过或拦截 / 生效 / 异常） |
//! | `JudgeEngine` | 组合上述，执行「判定 + 原子生效」，并通过回调广播事件 |
//! | `SkillSet` | 技能目录：以名索引管理一组规则，并委托 `JudgeEngine` 判定 |
//!
//! 并发安全：
//! - `ResourceLedger` 用 `AtomicU64` + CAS，无锁且保证总额守恒；
//! - `CooldownTracker` 用 `Mutex<HashMap>` 保证 check-and-set 原子；
//! - `JudgeEngine::trigger` 额外用一把提交锁，保证「判定 → 提交」跨组件一致，
//!   杜绝「冷却已 arm 但资源不足」这类部分提交的中间态。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ───────────────────────────── 时间抽象 ─────────────────────────────

/// 时间来源。`elapsed` 返回自某固定纪元起经过的时长，便于测试注入可控时间。
pub trait Clock: Send + Sync {
    fn elapsed(&self) -> Duration;
}

/// 真实时钟：基于 Unix 纪元。
pub struct SystemClock;

impl Clock for SystemClock {
    fn elapsed(&self) -> Duration {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
    }
}

/// 测试用时钟：时间可读写，便于精确驱动冷却逻辑。
pub struct MockClock {
    t: Arc<Mutex<Duration>>,
}

impl MockClock {
    pub fn new(start: Duration) -> Self {
        Self {
            t: Arc::new(Mutex::new(start)),
        }
    }

    /// 快进时间，用于模拟冷却结束。
    pub fn advance(&self, d: Duration) {
        let mut g = self.t.lock().unwrap();
        *g += d;
    }
}

impl Clock for MockClock {
    fn elapsed(&self) -> Duration {
        *self.t.lock().unwrap()
    }
}

// ───────────────────────────── 异常类型 ─────────────────────────────

/// 判定失败的可区分原因。所有失败路径都必须归入此枚举，便于调用方精确处理。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JudgeError {
    /// 技能仍在冷却中，附剩余冷却时长。
    CooldownActive { remaining: Duration },
    /// 资源不足，附当前余额与所需量。
    InsufficientResource { have: u64, need: u64 },
    /// 目标状态不符，附期望值与实际值。
    StateMismatch { expected: String, actual: String },
    /// 目录中找不到指定名称的技能（按名触发/判定时）。
    UnknownSkill(String),
    /// 组件内部错误（如锁 poisoned）。
    Internal(String),
}

impl std::fmt::Display for JudgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JudgeError::CooldownActive { remaining } => {
                write!(f, "skill on cooldown, remaining {:?}", remaining)
            }
            JudgeError::InsufficientResource { have, need } => {
                write!(f, "insufficient resource: have {}, need {}", have, need)
            }
            JudgeError::StateMismatch { expected, actual } => {
                write!(f, "state mismatch: expected `{}`, actual `{}`", expected, actual)
            }
            JudgeError::UnknownSkill(s) => write!(f, "unknown skill: `{}`", s),
            JudgeError::Internal(m) => write!(f, "internal error: {}", m),
        }
    }
}

// ───────────────────────────── 冷却追踪 ─────────────────────────────

/// 技能冷却追踪器：内部用 `Mutex<HashMap>` 保存每个技能上次触发时刻，
/// 提供只读 `is_ready` / `remaining` 与原子 `arm`（检查并设置）。
#[derive(Clone)]
pub struct CooldownTracker {
    inner: Arc<Mutex<HashMap<String, Duration>>>,
    clock: Arc<dyn Clock>,
}

impl CooldownTracker {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            clock,
        }
    }

    /// 是否已过冷却（只读，不修改状态）。
    pub fn is_ready(&self, skill: &str, cooldown: Duration) -> bool {
        let now = self.clock.elapsed();
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match g.get(skill) {
            Some(last) => now.saturating_sub(*last) >= cooldown,
            None => true,
        }
    }

    /// 距冷却结束的剩余时长（已过冷却则为 ZERO）。
    pub fn remaining(&self, skill: &str, cooldown: Duration) -> Duration {
        let now = self.clock.elapsed();
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match g.get(skill) {
            Some(last) => cooldown.saturating_sub(now.saturating_sub(*last)),
            None => Duration::ZERO,
        }
    }

    /// 原子地「检查并标记」冷却：若仍在冷却返回 `Err`，否则落点并返回 `Ok`。
    /// 锁内完成 check-and-set，保证并发下同一技能在冷却内只会被 arm 一次。
    pub fn arm(&self, skill: &str, cooldown: Duration) -> Result<(), JudgeError> {
        let now = self.clock.elapsed();
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(last) = g.get(skill) {
            let since = now.saturating_sub(*last);
            if since < cooldown {
                return Err(JudgeError::CooldownActive {
                    remaining: cooldown - since,
                });
            }
        }
        g.insert(skill.to_string(), now);
        Ok(())
    }

    /// 测试辅助：清除某技能的冷却记录。
    #[cfg(test)]
    pub fn reset(&self, skill: &str) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(skill);
    }
}

// ───────────────────────────── 资源账本 ─────────────────────────────

/// 资源账本：用 `AtomicU64` 保存余额，`try_consume` 以 CAS 无锁原子扣减，
/// 保证任意并发下余额不会为负、总额守恒。
#[derive(Clone)]
pub struct ResourceLedger {
    balance: Arc<AtomicU64>,
}

impl ResourceLedger {
    pub fn new(initial: u64) -> Self {
        Self {
            balance: Arc::new(AtomicU64::new(initial)),
        }
    }

    /// 当前余额。
    pub fn available(&self) -> u64 {
        self.balance.load(Ordering::SeqCst)
    }

    /// 补充资源（如回合恢复）。
    pub fn replenish(&self, amount: u64) {
        self.balance.fetch_add(amount, Ordering::SeqCst);
    }

    /// 仅检查是否足以支付（不修改状态）。
    pub fn can_afford(&self, cost: u64) -> bool {
        self.available() >= cost
    }

    /// 原子扣减：不足返回 `Err`；CAS 失败则重试，确保不会超发。
    pub fn try_consume(&self, cost: u64) -> Result<(), JudgeError> {
        loop {
            let have = self.balance.load(Ordering::SeqCst);
            if have < cost {
                return Err(JudgeError::InsufficientResource { have, need: cost });
            }
            if self
                .balance
                .compare_exchange(have, have - cost, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Ok(());
            }
            // CAS 失败说明被其他线程抢先，重新循环读取最新余额。
        }
    }
}

// ───────────────────────────── 目标状态 ─────────────────────────────

/// 目标状态提供者：仅负责返回「目标当前状态」字符串。
/// 解耦具体状态来源（数据库 / 内存 / 远程服务均可实现此 trait）。
pub trait StateProvider: Send + Sync {
    fn current_state(&self) -> String;
}

/// 恒定状态（测试 / 无状态场景用）。
pub struct ConstState(pub String);

impl StateProvider for ConstState {
    fn current_state(&self) -> String {
        self.0.clone()
    }
}

// ───────────────────────────── 规则数据 ─────────────────────────────

/// 一个技能的前置条件集合（纯数据，不含行为）。
#[derive(Debug, Clone, PartialEq)]
pub struct SkillRule {
    /// 技能标识（冷却键）。
    pub name: String,
    /// 冷却时长：两次触发最小间隔。
    pub cooldown: Duration,
    /// 触发消耗的资源量。
    pub cost: u64,
    /// 触发要求的目标状态；`None` 表示不检查状态。
    pub required_state: Option<String>,
}

// ───────────────────────────── 状态机事件 ─────────────────────────────

/// 判定生命周期中的状态机事件，由引擎通过回调广播。
/// 顺序示例（成功）：PreCheck → CooldownPassed → StatePassed →
/// ResourcePassed → Approved → Triggered。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JudgeEvent {
    PreCheck { skill: String },
    CooldownPassed { skill: String },
    CooldownBlocked { skill: String, remaining: Duration },
    ResourcePassed { skill: String },
    ResourceBlocked { skill: String, have: u64, need: u64 },
    StatePassed { skill: String },
    StateBlocked { skill: String, expected: String, actual: String },
    Approved { skill: String },
    Triggered { skill: String, remaining: u64 },
    Error { skill: String, reason: String },
}

// ───────────────────────────── 判定引擎 ─────────────────────────────

/// 技能判定引擎：组合冷却 / 资源 / 状态三类组件，对外提供「判定」与「触发」两类入口，
/// 并通过回调机制广播状态机事件。
pub struct JudgeEngine {
    cooldown: CooldownTracker,
    resources: ResourceLedger,
    state: Arc<dyn StateProvider>,
    callbacks: Mutex<Vec<Box<dyn Fn(&JudgeEvent) + Send + Sync>>>,
    /// 提交锁：串起「判定 → 提交」，避免跨组件部分提交。
    commit: Mutex<()>,
}

impl JudgeEngine {
    /// 使用真实时钟构造。
    pub fn new(initial_resources: u64, state: Arc<dyn StateProvider>) -> Self {
        Self::with_clock(initial_resources, state, Arc::new(SystemClock))
    }

    /// 注入时钟构造（测试用），便于确定性驱动冷却。
    pub fn with_clock(
        initial_resources: u64,
        state: Arc<dyn StateProvider>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            cooldown: CooldownTracker::new(clock),
            resources: ResourceLedger::new(initial_resources),
            state,
            callbacks: Mutex::new(Vec::new()),
            commit: Mutex::new(()),
        }
    }

    /// 注册状态机事件回调（可多次注册，按注册顺序触发）。
    pub fn on_event(&self, cb: impl Fn(&JudgeEvent) + Send + Sync + 'static) {
        self.callbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Box::new(cb));
    }

    /// 当前资源余额（只读查询）。
    pub fn resources_available(&self) -> u64 {
        self.resources.available()
    }

    fn emit(&self, ev: JudgeEvent) {
        // 防御性：回调异常不应影响引擎主流程。
        let cbs = self.callbacks.lock().unwrap_or_else(|e| e.into_inner());
        for cb in cbs.iter() {
            cb(&ev);
        }
    }

    /// 纯前置验证：检查冷却 / 资源充足 / 目标状态，但**不修改任何状态**。
    /// 适用于「能否释放」的只读查询。失败返回对应 `JudgeError` 并广播拦截事件。
    pub fn judge(&self, rule: &SkillRule) -> Result<(), JudgeError> {
        self.emit(JudgeEvent::PreCheck {
            skill: rule.name.clone(),
        });

        if !self.cooldown.is_ready(&rule.name, rule.cooldown) {
            let remaining = self.cooldown.remaining(&rule.name, rule.cooldown);
            self.emit(JudgeEvent::CooldownBlocked {
                skill: rule.name.clone(),
                remaining,
            });
            return Err(JudgeError::CooldownActive { remaining });
        }
        self.emit(JudgeEvent::CooldownPassed {
            skill: rule.name.clone(),
        });

        if !self.resources.can_afford(rule.cost) {
            let have = self.resources.available();
            self.emit(JudgeEvent::ResourceBlocked {
                skill: rule.name.clone(),
                have,
                need: rule.cost,
            });
            return Err(JudgeError::InsufficientResource {
                have,
                need: rule.cost,
            });
        }
        self.emit(JudgeEvent::ResourcePassed {
            skill: rule.name.clone(),
        });

        if let Some(req) = &rule.required_state {
            let actual = self.state.current_state();
            if &actual != req {
                self.emit(JudgeEvent::StateBlocked {
                    skill: rule.name.clone(),
                    expected: req.clone(),
                    actual: actual.clone(),
                });
                return Err(JudgeError::StateMismatch {
                    expected: req.clone(),
                    actual,
                });
            }
        }
        self.emit(JudgeEvent::StatePassed {
            skill: rule.name.clone(),
        });
        self.emit(JudgeEvent::Approved {
            skill: rule.name.clone(),
        });
        Ok(())
    }

    /// 判定 + 原子生效：在提交锁内完成「冷却 arm → 状态核对 → 资源扣减」，
    /// 全部通过后才算触发成功。任何一步失败立即返回 `Err` 并广播 `Error` 事件。
    pub fn trigger(&self, rule: &SkillRule) -> Result<TriggerOutcome, JudgeError> {
        let _guard = self.commit.lock().unwrap_or_else(|e| e.into_inner());
        self.emit(JudgeEvent::PreCheck {
            skill: rule.name.clone(),
        });

        if let Err(e) = self.cooldown.arm(&rule.name, rule.cooldown) {
            if let JudgeError::CooldownActive { remaining } = &e {
                self.emit(JudgeEvent::CooldownBlocked {
                    skill: rule.name.clone(),
                    remaining: *remaining,
                });
            }
            self.emit(JudgeEvent::Error {
                skill: rule.name.clone(),
                reason: e.to_string(),
            });
            return Err(e);
        }
        self.emit(JudgeEvent::CooldownPassed {
            skill: rule.name.clone(),
        });

        if let Some(req) = &rule.required_state {
            let actual = self.state.current_state();
            if &actual != req {
                self.emit(JudgeEvent::StateBlocked {
                    skill: rule.name.clone(),
                    expected: req.clone(),
                    actual: actual.clone(),
                });
                self.emit(JudgeEvent::Error {
                    skill: rule.name.clone(),
                    reason: format!("state mismatch: expected {}, actual {}", req, actual),
                });
                return Err(JudgeError::StateMismatch {
                    expected: req.clone(),
                    actual,
                });
            }
        }
        self.emit(JudgeEvent::StatePassed {
            skill: rule.name.clone(),
        });

        if let Err(e) = self.resources.try_consume(rule.cost) {
            if let JudgeError::InsufficientResource { have, need } = &e {
                self.emit(JudgeEvent::ResourceBlocked {
                    skill: rule.name.clone(),
                    have: *have,
                    need: *need,
                });
            }
            self.emit(JudgeEvent::Error {
                skill: rule.name.clone(),
                reason: e.to_string(),
            });
            return Err(e);
        }
        self.emit(JudgeEvent::ResourcePassed {
            skill: rule.name.clone(),
        });
        self.emit(JudgeEvent::Approved {
            skill: rule.name.clone(),
        });

        let remaining = self.resources.available();
        self.emit(JudgeEvent::Triggered {
            skill: rule.name.clone(),
            remaining,
        });
        Ok(TriggerOutcome {
            skill: rule.name.clone(),
            consumed: rule.cost,
            remaining_resources: remaining,
        })
    }
}

/// 触发成功的产出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerOutcome {
    pub skill: String,
    pub consumed: u64,
    pub remaining_resources: u64,
}

// ───────────────────────────── 技能目录 ─────────────────────────────

/// 技能目录：以「名称 → `SkillRule`」索引管理一组技能规则，并共享同一个判定引擎。
///
/// 单一职责：本类型只负责规则的新增 / 移除 / 按名查询与转发触发，
/// 具体的冷却 / 资源 / 状态判定逻辑仍完全委托给 `JudgeEngine`，
/// 自身不持有任何判定状态，从而保持高内聚、低耦合。
///
/// 并发安全：`rules` 由 `Mutex<HashMap>` 保护，注册与查询可安全并发；
/// 真正的并发临界区（资源扣减、冷却 arm）在 `JudgeEngine` 内部，已另有保障。
#[derive(Clone)]
pub struct SkillSet {
    engine: Arc<JudgeEngine>,
    rules: Arc<Mutex<HashMap<String, SkillRule>>>,
}

impl SkillSet {
    /// 基于一个判定引擎构造目录。
    pub fn new(engine: Arc<JudgeEngine>) -> Self {
        Self {
            engine,
            rules: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 登记 / 覆盖一个技能规则。
    pub fn register(&self, rule: SkillRule) {
        self.rules
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(rule.name.clone(), rule);
    }

    /// 移除一个技能规则；不存在则无操作。
    pub fn unregister(&self, name: &str) {
        self.rules
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name);
    }

    /// 是否存在该技能。
    pub fn contains(&self, name: &str) -> bool {
        self.rules
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(name)
    }

    /// 按名查询规则副本（纯数据，克隆返回）。
    pub fn get(&self, name: &str) -> Option<SkillRule> {
        self.rules
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .cloned()
    }

    /// 当前已登记技能数量。
    pub fn len(&self) -> usize {
        self.rules
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// 是否没有任何技能。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 按名纯判定（只读预检）。
    pub fn judge(&self, name: &str) -> Result<(), JudgeError> {
        let rule = self
            .get(name)
            .ok_or_else(|| JudgeError::UnknownSkill(name.to_string()))?;
        self.engine.judge(&rule)
    }

    /// 按名判定 + 原子生效。
    pub fn trigger(&self, name: &str) -> Result<TriggerOutcome, JudgeError> {
        let rule = self
            .get(name)
            .ok_or_else(|| JudgeError::UnknownSkill(name.to_string()))?;
        self.engine.trigger(&rule)
    }

    /// 取共享引擎（便于注册统一事件回调）。
    pub fn engine(&self) -> &Arc<JudgeEngine> {
        &self.engine
    }
}

// ───────────────────────────── 单元测试 ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str, cooldown: Duration, cost: u64, state: Option<&str>) -> SkillRule {
        SkillRule {
            name: name.to_string(),
            cooldown,
            cost,
            required_state: state.map(|s| s.to_string()),
        }
    }

    // ---- 冷却追踪 ----
    #[test]
    fn cooldown_arm_then_blocked_until_elapsed() {
        let clock = Arc::new(MockClock::new(Duration::ZERO));
        let cd = CooldownTracker::new(clock.clone());
        assert!(cd.is_ready("fire", Duration::from_secs(10)));
        cd.arm("fire", Duration::from_secs(10)).unwrap();
        assert!(!cd.is_ready("fire", Duration::from_secs(10)));
        // 未到冷却：arm 失败
        let e = cd.arm("fire", Duration::from_secs(10)).unwrap_err();
        assert!(matches!(e, JudgeError::CooldownActive { .. }));
        // 推进时间越过冷却
        clock.advance(Duration::from_secs(10));
        assert!(cd.is_ready("fire", Duration::from_secs(10)));
        cd.arm("fire", Duration::from_secs(10)).unwrap(); // 再次成功
    }

    #[test]
    fn cooldown_remaining_shrinks_over_time() {
        let clock = Arc::new(MockClock::new(Duration::ZERO));
        let cd = CooldownTracker::new(clock.clone());
        cd.arm("s", Duration::from_secs(60)).unwrap();
        assert_eq!(cd.remaining("s", Duration::from_secs(60)), Duration::from_secs(60));
        clock.advance(Duration::from_secs(20));
        assert_eq!(cd.remaining("s", Duration::from_secs(60)), Duration::from_secs(40));
    }

    // ---- 资源账本 ----
    #[test]
    fn resource_consume_success_and_insufficient() {
        let r = ResourceLedger::new(10);
        r.try_consume(4).unwrap();
        assert_eq!(r.available(), 6);
        r.try_consume(6).unwrap();
        assert_eq!(r.available(), 0);
        assert!(matches!(
            r.try_consume(1),
            Err(JudgeError::InsufficientResource { have: 0, need: 1 })
        ));
    }

    #[test]
    fn resource_concurrency_conserves_total() {
        // 多线程各尝试扣 1，余额 2000 应被精确耗尽，绝不超发。
        let r = ResourceLedger::new(2000);
        let mut handles = Vec::new();
        for _ in 0..50 {
            let r = r.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    let _ = r.try_consume(1);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(r.available(), 0);
    }

    // ---- 目标状态 ----
    #[test]
    fn const_state_provider_returns_fixed_value() {
        let s = ConstState("battle".into());
        assert_eq!(s.current_state(), "battle");
    }

    // ---- 纯判定 judge 不修改状态 ----
    #[test]
    fn judge_pure_check_does_not_mutate_state() {
        let clock = Arc::new(MockClock::new(Duration::ZERO));
        let e = JudgeEngine::with_clock(100, Arc::new(ConstState("idle".into())), clock);
        let r = rule("heal", Duration::from_secs(1), 10, Some("idle"));
        assert!(e.judge(&r).is_ok());
        // judge 不改资源、不记冷却
        assert_eq!(e.resources_available(), 100);
        assert!(e.cooldown.is_ready("heal", Duration::from_secs(1)));
    }

    #[test]
    fn judge_reports_state_mismatch_without_consuming() {
        let e = JudgeEngine::new(100, Arc::new(ConstState("battle".into())));
        let r = rule("heal", Duration::ZERO, 10, Some("idle"));
        let err = e.judge(&r).unwrap_err();
        assert!(matches!(
            err,
            JudgeError::StateMismatch {
                expected,
                actual
            } if expected == "idle" && actual == "battle"
        ));
        assert_eq!(e.resources_available(), 100);
    }

    // ---- 触发成功路径 + 事件回调 ----
    #[test]
    fn trigger_success_emits_full_event_chain_and_consumes() {
        let e = Arc::new(JudgeEngine::new(
            100,
            Arc::new(ConstState("idle".into())),
        ));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        e.on_event(move |ev| seen2.lock().unwrap().push(ev.clone()));

        let r = rule("fire", Duration::from_secs(5), 30, Some("idle"));
        let out = e.trigger(&r).unwrap();
        assert_eq!(out.consumed, 30);
        assert_eq!(out.remaining_resources, 70);
        assert_eq!(e.resources_available(), 70);

        let events = seen.lock().unwrap();
        let names: Vec<_> = events.iter().map(|e| match e {
            JudgeEvent::PreCheck { .. } => "PreCheck",
            JudgeEvent::CooldownPassed { .. } => "CooldownPassed",
            JudgeEvent::StatePassed { .. } => "StatePassed",
            JudgeEvent::ResourcePassed { .. } => "ResourcePassed",
            JudgeEvent::Approved { .. } => "Approved",
            JudgeEvent::Triggered { .. } => "Triggered",
            other => panic!("unexpected event: {:?}", other),
        }).collect();
        assert_eq!(
            names,
            vec!["PreCheck", "CooldownPassed", "StatePassed", "ResourcePassed", "Approved", "Triggered"]
        );
    }

    // ---- 触发失败路径：冷却拦截 ----
    #[test]
    fn trigger_blocked_by_cooldown_emits_events_and_no_consume() {
        let clock = Arc::new(MockClock::new(Duration::ZERO));
        let e = Arc::new(JudgeEngine::with_clock(
            100,
            Arc::new(ConstState("idle".into())),
            clock,
        ));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        e.on_event(move |ev| seen2.lock().unwrap().push(ev.clone()));

        let r = rule("fire", Duration::from_secs(10), 10, Some("idle"));
        assert!(e.trigger(&r).is_ok());
        // 立即再次触发：冷却中
        let err = e.trigger(&r).unwrap_err();
        assert!(matches!(err, JudgeError::CooldownActive { .. }));
        // 资源只扣一次
        assert_eq!(e.resources_available(), 90);

        let has_blocked = seen
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, JudgeEvent::CooldownBlocked { .. }));
        assert!(has_blocked);
        let has_error = seen
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, JudgeEvent::Error { .. }));
        assert!(has_error);
    }

    // ---- 触发失败路径：资源不足 ----
    #[test]
    fn trigger_blocked_by_resource_emits_resource_blocked() {
        let e = Arc::new(JudgeEngine::new(
            5,
            Arc::new(ConstState("idle".into())),
        ));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        e.on_event(move |ev| seen2.lock().unwrap().push(ev.clone()));

        let r = rule("big", Duration::ZERO, 10, Some("idle"));
        assert!(matches!(
            e.trigger(&r),
            Err(JudgeError::InsufficientResource { have: 5, need: 10 })
        ));
        assert!(seen
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, JudgeEvent::ResourceBlocked { .. })));
    }

    // ---- 触发失败路径：状态不符 ----
    #[test]
    fn trigger_blocked_by_state_emits_state_blocked() {
        let e = Arc::new(JudgeEngine::new(
            100,
            Arc::new(ConstState("battle".into())),
        ));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        e.on_event(move |ev| seen2.lock().unwrap().push(ev.clone()));

        let r = rule("heal", Duration::ZERO, 1, Some("idle"));
        assert!(matches!(
            e.trigger(&r),
            Err(JudgeError::StateMismatch { expected, actual }) if expected == "idle" && actual == "battle"
        ));
        assert!(seen
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, JudgeEvent::StateBlocked { .. })));
    }

    // ---- 并发安全：多线程触发不同技能，资源精确守恒 ----
    #[test]
    fn trigger_concurrent_distinct_skills_is_safe_and_conserves() {
        let e = Arc::new(JudgeEngine::new(
            1_000_000,
            Arc::new(ConstState("idle".into())),
        ));
        let mut handles = Vec::new();
        for i in 0..40 {
            let e = e.clone();
            handles.push(std::thread::spawn(move || {
                let r = SkillRule {
                    name: format!("skill_{}", i),
                    cooldown: Duration::ZERO,
                    cost: 1,
                    required_state: Some("idle".into()),
                };
                for _ in 0..100 {
                    let _ = e.trigger(&r);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // 40 技能 * 100 次 * 1 资源 = 4000 消耗
        assert_eq!(e.resources_available(), 1_000_000 - 4000);
    }

    // ---- 并发安全：同一技能在冷却内只生效一次（提交锁保证） ----
    #[test]
    fn trigger_concurrent_same_skill_respects_cooldown() {
        let clock = Arc::new(MockClock::new(Duration::ZERO));
        let e = Arc::new(JudgeEngine::with_clock(
            1_000_000,
            Arc::new(ConstState("idle".into())),
            clock,
        ));
        let mut handles = Vec::new();
        for _ in 0..20 {
            let e = e.clone();
            handles.push(std::thread::spawn(move || {
                let r = SkillRule {
                    name: "uniq".into(),
                    cooldown: Duration::from_secs(60),
                    cost: 1,
                    required_state: Some("idle".into()),
                };
                // 20 个线程同时抢同一技能，冷却 60s，仅第一个应成功
                let _ = e.trigger(&r);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // 无论多少线程并发，冷却内只扣 1 点
        assert_eq!(e.resources_available(), 1_000_000 - 1);
    }

    // ---- 技能目录 SkillSet ----
    #[test]
    fn skillset_register_and_lookup() {
        let set = SkillSet::new(Arc::new(JudgeEngine::new(
            10,
            Arc::new(ConstState("idle".into())),
        )));
        assert!(set.is_empty());
        let r = rule("fire", Duration::from_secs(5), 30, Some("idle"));
        set.register(r.clone());
        assert!(set.contains("fire"));
        assert_eq!(set.len(), 1);
        assert_eq!(set.get("fire"), Some(r));
        set.unregister("fire");
        assert!(set.is_empty());
        assert!(!set.contains("fire"));
    }

    #[test]
    fn skillset_judge_and_trigger_unknown_returns_unknown_skill() {
        let set = SkillSet::new(Arc::new(JudgeEngine::new(
            10,
            Arc::new(ConstState("idle".into())),
        )));
        assert!(matches!(
            set.judge("ghost"),
            Err(JudgeError::UnknownSkill(n)) if n == "ghost"
        ));
        assert!(matches!(
            set.trigger("ghost"),
            Err(JudgeError::UnknownSkill(n)) if n == "ghost"
        ));
    }

    #[test]
    fn skillset_trigger_by_name_consumes_and_emits() {
        let set = Arc::new(SkillSet::new(Arc::new(JudgeEngine::new(
            100,
            Arc::new(ConstState("idle".into())),
        ))));
        let seen = Arc::new(Mutex::new(0usize));
        let seen2 = seen.clone();
        set.engine().on_event(move |ev| {
            if matches!(ev, JudgeEvent::Triggered { .. }) {
                *seen2.lock().unwrap() += 1;
            }
        });
        set.register(rule("fire", Duration::from_secs(5), 30, Some("idle")));
        let out = set.trigger("fire").unwrap();
        assert_eq!(out.consumed, 30);
        assert_eq!(out.remaining_resources, 70);
        assert_eq!(*seen.lock().unwrap(), 1);
        // 冷却内同名再次触发被拦截
        assert!(matches!(
            set.trigger("fire"),
            Err(JudgeError::CooldownActive { .. })
        ));
    }

    #[test]
    fn skillset_concurrent_register_and_trigger_is_safe() {
        let set = Arc::new(SkillSet::new(Arc::new(JudgeEngine::new(
            1_000_000,
            Arc::new(ConstState("idle".into())),
        ))));
        // 预登记 40 个技能
        for i in 0..40 {
            set.register(SkillRule {
                name: format!("s{}", i),
                cooldown: Duration::ZERO,
                cost: 1,
                required_state: Some("idle".into()),
            });
        }
        let mut handles = Vec::new();
        for i in 0..40 {
            let set = set.clone();
            handles.push(std::thread::spawn(move || {
                let name = format!("s{}", i);
                for _ in 0..100 {
                    // 边注册边触发，模拟动态配置热更新 + 高频触发
                    if i % 7 == 0 {
                        set.register(SkillRule {
                            name: name.clone(),
                            cooldown: Duration::ZERO,
                            cost: 1,
                            required_state: Some("idle".into()),
                        });
                    }
                    let _ = set.trigger(&name);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(set.engine().resources_available(), 1_000_000 - 4000);
    }
}
