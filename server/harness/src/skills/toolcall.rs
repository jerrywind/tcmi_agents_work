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
    /// 专属 owner（仅该 capability 的 agent 可调用）；None 表示全局可用
    pub owner: Option<Capability>,
    pub executor: SkillFn,
}

impl Skill {
    pub fn new(
        name: &str,
        description: &str,
        parameters: Value,
        executor: SkillFn,
    ) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
            owner: None,
            executor,
        }
    }

    /// 设置专属 owner
    pub fn with_owner(mut self, cap: Capability) -> Self {
        self.owner = Some(cap);
        self
    }
}

/// 按名字分发执行（异步）
pub async fn dispatch(skills: &[Skill], name: &str, args: &Value) -> Result<Value> {
    let skill = skills
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| anyhow::anyhow!("未知技能: {name}"))?;
    (skill.executor)(args).await
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

/// 构造一个通过 MCP 调用的技能
pub fn mcp_skill(
    name: &str,
    description: &str,
    parameters: Value,
    mcp_url: String,
    client: reqwest::Client,
) -> Skill {
    let tool = name.to_string();
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
