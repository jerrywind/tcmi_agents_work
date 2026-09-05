//! 工具调用实现：HTTP 端点与 MCP server
//!
//! 复刻 backend `app/skills/toolcall.py`。
//! - `http_skill`：把请求转发到外部 HTTP 端点（如知识库 / 方剂库 API）
//! - `mcp_skill`：通过 MCP（Streamable HTTP）调用外部工具（见 `crate::mcp`）
//!
//! 执行器为异步闭包（返回 `BoxFuture`），避免在 async 栈内嵌套 `block_on`。

use crate::mcp;
use crate::model::Capability;
use anyhow::Result;
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::Arc;

/// 异步工具执行器：输入参数 -> 输出结果
pub type SkillFn = Arc<dyn Fn(&Value) -> BoxFuture<'static, Result<Value>> + Send + Sync>;

/// 一个可调用技能
#[derive(Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// JSON Schema 形式的参数定义
    pub parameters: Value,
    /// 可用该技能的 capability 集合（**空集合 = 全局可用**）。
    ///
    /// 之所以是集合而非单个 owner：默认 `standard` 档把旧的一步到位
    /// `treatment` 拆成了「立法 → 用药 → 开方」，一个工具往往要同时服务
    /// 其中好几步（方剂检索对用药、开方都有用）。一对一 owner 会让
    /// 拆分后的步骤一件专属工具都拿不到，只能靠模型记忆——
    /// 这正是「技能调用偏少 / 方剂药味记错」的配置层根因。
    pub owners: Vec<Capability>,
    pub executor: SkillFn,
}

impl Skill {
    pub fn new(name: &str, description: &str, parameters: Value, executor: SkillFn) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
            owners: Vec::new(),
            executor,
        }
    }

    /// 设置单个专属 owner
    pub fn with_owner(mut self, cap: Capability) -> Self {
        if !self.owners.contains(&cap) {
            self.owners.push(cap);
        }
        self
    }

    /// 设置多个专属 owner（方剂检索这类要横跨治疗期多步的工具）
    pub fn with_owners<I: IntoIterator<Item = Capability>>(mut self, caps: I) -> Self {
        for c in caps {
            if !self.owners.contains(&c) {
                self.owners.push(c);
            }
        }
        self
    }

    /// 该 capability 是否可用此技能（无 owner 约束即全局可见）
    pub fn visible_to(&self, cap: Capability) -> bool {
        self.owners.is_empty() || self.owners.contains(&cap)
    }
}

/// 参数里注入的「调用方 capability」字段名
///
/// `SkillFn` 只接收 `&Value`，技能无从知道是哪个 agent 在调用自己。
/// 而「按知识域检索」必须知道——开方 agent 查方书、切诊 agent 查脉学，
/// 同一个 `tcm-rag` 技能要按调用方给出不同的检索域。
/// 与其改 `SkillFn` 签名（会波及全部技能与 MCP 适配器），
/// 不如在分发时把调用方塞进参数：技能需要就读，不需要则完全无感。
pub const CALLER_FIELD: &str = "_caller";

/// 按名字分发执行（异步）
///
/// `caller` 为调用方 capability（`POST /skills` 手动调用时传 `None`），
/// 会以 [`CALLER_FIELD`] 注入参数供技能按需读取。
pub async fn dispatch(
    skills: &[Skill],
    name: &str,
    args: &Value,
    caller: Option<Capability>,
) -> Result<Value> {
    let skill = skills
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| anyhow::anyhow!("未知技能: {name}"))?;
    match (caller, args) {
        (Some(cap), Value::Object(_)) => {
            let mut args = args.clone();
            if let Value::Object(m) = &mut args {
                m.insert(CALLER_FIELD.into(), json!(cap.slug()));
            }
            (skill.executor)(&args).await
        }
        _ => (skill.executor)(args).await,
    }
}

/// 构造一个转发到 HTTP 端点的技能
pub fn http_skill(
    name: &str,
    description: &str,
    parameters: Value,
    endpoint: String,
    client: reqwest::Client,
) -> Skill {
    let exec: SkillFn = Arc::new(move |args: &Value| {
        let endpoint = endpoint.clone();
        let client = client.clone();
        let args = args.clone();
        Box::pin(async move {
            let resp = client
                .post(&endpoint)
                .json(&args)
                .send()
                .await?
                .error_for_status()?
                .json::<Value>()
                .await?;
            Ok(resp)
        })
    });
    Skill::new(name, description, parameters, exec)
}

/// 构造一个通过 MCP 调用的技能（显示名与远端工具名不同时使用）
///
/// MCP server 挂载出来的工具统一命名为 `mcp__<client>__<tool>`，
/// 但调用远端时仍须用**原始工具名**，故两者分开传。
pub fn mcp_skill_named(
    name: &str,
    remote_tool: &str,
    description: &str,
    parameters: Value,
    mcp_url: String,
    client: reqwest::Client,
) -> Skill {
    let tool = remote_tool.to_string();
    let exec: SkillFn = Arc::new(move |args: &Value| {
        let mcp_url = mcp_url.clone();
        let client = client.clone();
        let tool = tool.clone();
        let args = args.clone();
        Box::pin(async move {
            let result = mcp::call_tool(&client, &mcp_url, &tool, &args).await?;
            Ok(json!(result))
        })
    });
    Skill::new(name, description, parameters, exec)
}

/// 构造一个通过 MCP 调用的技能（显示名 = 远端工具名）
pub fn mcp_skill(
    name: &str,
    description: &str,
    parameters: Value,
    mcp_url: String,
    client: reqwest::Client,
) -> Skill {
    mcp_skill_named(name, name, description, parameters, mcp_url, client)
}
