//! 望诊 sub-agent
//!
//! 复刻 backend `app/agents/inspection.py`：观察神色形态、舌象（图片或文字描述），
//! 通过 LLM 抽取结构化特征，并叠加 `resources/keywords.yaml` 的证据匹配。

use crate::agents::base::{AgentContext, SubAgent};
use crate::model::{Capability, Message};
use anyhow::Result;
use async_trait::async_trait;

pub struct InspectionAgent;

#[async_trait]
impl SubAgent for InspectionAgent {
    fn capability(&self) -> Capability {
        Capability::Inspection
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        messages: &[Message],
        payload: &serde_json::Value,
    ) -> Result<String> {
        let system = &ctx.resources.prompts.inspection;
        // 用 chat_with_tools 而非 chat_completion：让模型能调用 tcm-vision 等技能。
        // 该 capability 无可用技能时，内部会自动退化为普通补全。

        // 前端上传的舌苔 / 左手手相 / 右手手相图片（data URL）随 `payload.images` 传来，
        // 作为视觉输入挂到首条 user 消息上喂给多模态模型。
        // 每张图前插入一句中文标签（舌苔照片 / 左手手相照片 / 右手手相照片），
        // 否则模型只看到若干图片、无法区分哪张对应哪只手——尤其手相分左右手时。
        // 无图时 `chat_with_tools_imgs` 退化为普通 `chat_with_tools`，行为不变。
        let label_of = |kind: &str| -> &str {
            match kind {
                "tongue" => "舌苔照片",
                "palm_left" => "左手手相照片",
                "palm_right" => "右手手相照片",
                _ => "望诊照片",
            }
        };
        let images: Vec<serde_json::Value> = payload
            .get("images")
            .and_then(|v| v.as_array())
            .map(|arr| {
                let mut parts: Vec<serde_json::Value> = Vec::new();
                let mut any = false;
                for img in arr.iter() {
                    let url = match img.get("data_url").and_then(|u| u.as_str()) {
                        Some(u) => u,
                        None => continue,
                    };
                    any = true;
                    let kind = img.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                    parts.push(json!({"type": "text", "text": format!("【{}】", label_of(kind))}));
                    parts.push(json!({"type": "image_url", "image_url": { "url": url }}));
                }
                if any {
                    parts.insert(
                        0,
                        json!({"type": "text", "text": "以下是随主诉上传的望诊图片，请结合文字描述一并分析："}),
                    );
                }
                parts
            })
            .unwrap_or_default();

        let mut out = ctx
            .caller()
            .chat_with_tools_imgs(system, messages, Capability::Inspection, &images)
            .await?;

        // 关键词证据叠加：从最新用户消息匹配舌象/面色证据
        if let Some(last) = messages.iter().rev().find(|m| m.role == "user") {
            let evidence = crate::agents::match_keywords(&ctx.resources, &last.content);
            if !evidence.is_empty() {
                out.push_str("\n\n[望诊证据] ");
                out.push_str(&evidence.join("；"));
            }
        }
        Ok(out)
    }
}
