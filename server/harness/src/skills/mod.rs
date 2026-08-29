//! 技能/工具系统
//!
//! 复刻 backend `app/skills/`：每个 Skill 是一个可被 LLM tool-calling 调用的工具，
//! 可对接 MCP server 或普通 HTTP 端点。registry 用名字查找，`dispatch` 执行。

pub mod builtin;
pub mod toolcall;

pub use builtin::{build_default_registry, mount_mcp, mount_mcp_clients};
pub use toolcall::{dispatch, http_skill, mcp_skill, mcp_skill_named, Skill, SkillFn};

use crate::model::Capability;
use std::collections::HashMap;

/// 全局技能注册表（启动时根据配置/资源构建）
#[derive(Clone)]
pub struct SkillRegistry {
    map: HashMap<String, Skill>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn register(&mut self, skill: Skill) {
        self.map.insert(skill.name.clone(), skill);
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.map.get(name)
    }

    /// 全部技能，**按名称稳定排序**后返回。
    ///
    /// 内部用 HashMap 存储，直接 `values().collect()` 会得到不稳定的顺序，
    /// 导致 `GET /skills` 每次进程启动返回顺序都可能不同（同 `Registry::capabilities`）。
    pub fn all(&self) -> Vec<Skill> {
        let mut list: Vec<Skill> = self.map.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    /// 返回某 capability 可用（专属或全局）的技能
    pub fn for_capability(&self, cap: Capability) -> Vec<&Skill> {
        self.map
            .values()
            .filter(|s| s.owner.is_none() || s.owner == Some(cap))
            .collect()
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}
