//! 针灸外治 sub-agent
//!
//! 从 44 部针灸典籍（刺法灸法 / 腧穴考证 / 经络理论 / 时间针法 / 推拿按摩）
//! 检索取穴与手法依据。默认**不在激活流程里**（见 `routing.yaml` 的
//! `compatible` / `standard` 档位），需要针灸方案时显式启用。
//!
//! 为什么与开方分开：针灸与方药是两套治疗体系，合在「治疗」一步里
//! 模型常常只给方不给穴，或两者都给得很浅。

use crate::agents::base::{AgentContext, SubAgent};
use crate::model::{Capability, Message};
use anyhow::Result;
use async_trait::async_trait;

pub struct AcupunctureAgent;

#[async_trait]
impl SubAgent for AcupunctureAgent {
    fn capability(&self) -> Capability {
        Capability::Acupuncture
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],
        _payload: &serde_json::Value,
    ) -> Result<String> {
        // 规则层：无。取穴高度依赖具体证候与经络辨证，
        // 硬编码规则反而会误导，交给模型在针灸典籍里检索后综合。
        let system = &ctx.resources.prompts.acupuncture;
        let llm = ctx
            .caller()
            .chat_with_tools(system, messages, Capability::Acupuncture)
            .await?;
        Ok(llm)
    }
}
