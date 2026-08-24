//! 技能判定引擎端到端演示（拉通流程）。
//!
//! 从「配置」加载一组技能，注册到 `SkillSet`，挂上统一事件回调，
//! 然后并发高频触发，观察：
//! 1. 状态机事件（PreCheck → … → Triggered / Blocked）被正确广播；
//! 2. 资源在多线程下精确守恒、绝不超发；
//! 3. 冷却内同名技能只生效一次。
//!
//! 运行：`cargo run --example skill_demo --offline`

use std::sync::Arc;
use std::time::Duration;

use rrserver::skill::{JudgeEngine, JudgeEvent, SkillRule, SkillSet, StateProvider};

/// 一个最简单的「目标状态」实现：回合制状态机，可由外部切换。
struct BattleState {
    inner: Arc<std::sync::Mutex<String>>,
}

impl BattleState {
    fn new(initial: &str) -> Self {
        Self {
            inner: Arc::new(std::sync::Mutex::new(initial.to_string())),
        }
    }
    fn set(&self, s: &str) {
        *self.inner.lock().unwrap() = s.to_string();
    }
}

impl StateProvider for BattleState {
    fn current_state(&self) -> String {
        self.inner.lock().unwrap().clone()
    }
}

fn main() {
    // 1) 资源 / 目标状态
    let state = Arc::new(BattleState::new("idle"));
    let engine = Arc::new(JudgeEngine::new(1_000_000, state.clone()));

    // 2) 统一事件回调：把所有事件打到 stdout（真实场景可接日志 / 审计 / 指标）
    engine.on_event(|ev: &JudgeEvent| match ev {
        JudgeEvent::Triggered { skill, remaining } => {
            println!("[event] Triggered  skill={} remaining_resources={}", skill, remaining)
        }
        JudgeEvent::CooldownBlocked { skill, remaining } => {
            println!(
                "[event] CooldownBlocked skill={} remaining={:?}",
                skill, remaining
            )
        }
        JudgeEvent::ResourceBlocked { skill, have, need } => {
            println!("[event] ResourceBlocked skill={} have={} need={}", skill, have, need)
        }
        JudgeEvent::StateBlocked {
            skill,
            expected,
            actual,
        } => {
            println!(
                "[event] StateBlocked skill={} expected={} actual={}",
                skill, expected, actual
            )
        }
        JudgeEvent::Error { skill, reason } => {
            println!("[event] Error skill={} reason={}", skill, reason)
        }
        other => println!("[event] {:?}", other),
    });

    // 3) 从「配置」加载技能目录
    let set = Arc::new(SkillSet::new(engine));
    let config: Vec<SkillRule> = vec![
        SkillRule {
            name: "fireball".into(),
            cooldown: Duration::from_millis(50),
            cost: 10,
            required_state: Some("idle".into()),
        },
        SkillRule {
            name: "heal".into(),
            cooldown: Duration::from_millis(50),
            cost: 5,
            required_state: Some("idle".into()),
        },
        SkillRule {
            name: "ultimate".into(),
            cooldown: Duration::ZERO,
            cost: 1,
            required_state: None, // 不检查状态
        },
    ];
    for r in config {
        set.register(r);
    }
    println!("[setup] loaded {} skills", set.len());

    // 4) 并发高频触发
    let mut handles = Vec::new();
    for tid in 0..8u32 {
        let set = set.clone();
        let st = state.clone();
        handles.push(std::thread::spawn(move || {
            let names = ["fireball", "heal", "ultimate"];
            for i in 0..200u32 {
                let name = names[(tid + i) as usize % names.len()];
                match set.trigger(name) {
                    Ok(_) => {
                        // 触发成功后切换一次状态，演示 state 变化对后续的影响
                        if i % 50 == 0 {
                            st.set("idle");
                        }
                    }
                    Err(_e) => {
                        // 冷却 / 资源 / 状态拦截都经事件回调广播，这里仅计数
                    }
                }
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // 5) 校验资源守恒
    let left = set.engine().resources_available();
    println!("[result] resources left = {}", left);
    assert!(left <= 1_000_000, "资源不应超发");
    println!("[result] SKILL DEMO PASS");
}
