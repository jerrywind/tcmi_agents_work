//! 资源加载：把 resources/ 下的若干 YAML 合并成一个 ResourceBundle。
//!
//! 加载失败时返回详细错误，便于中医修改 YAML 后快速定位问题。

use crate::resources::model::*;
use anyhow::{Context, Result};
use std::path::Path;

/// 从给定目录加载全部资源文件。
///
/// 目录约定（文件名固定，便于维护）：
/// - syndromes.yaml        证候库
/// - questions.yaml        问诊问题库
/// - keywords.yaml         关键词证据映射
/// - safety.yaml           红色警戒（安全门）
/// - transformations.yaml  传变
/// - formulas.yaml         方剂库
/// - care.yaml             调护方案
/// - prompts.yaml          各 agent 提示词
/// - routing.yaml          当前激活的 agent 路由
// 逐字段赋值是刻意的：每个字段来自不同的 YAML，且各自带独立的错误上下文，
// 比一个巨大的结构体字面量更易读、改起来也不容易串行出错。
#[allow(clippy::field_reassign_with_default)]
pub fn load(dir: &Path) -> Result<ResourceBundle> {
    let mut bundle = ResourceBundle::default();

    bundle.syndromes =
        load_list::<Syndrome>(dir.join("syndromes.yaml")).context("加载 syndromes.yaml 失败")?;
    bundle.questions = load_list::<QuestionItem>(dir.join("questions.yaml"))
        .context("加载 questions.yaml 失败")?;
    bundle.keyword_evidence = load_list::<KeywordEvidence>(dir.join("keywords.yaml"))
        .context("加载 keywords.yaml 失败")?;
    bundle.contradictions = load_list_opt::<Contradiction>(dir.join("contradictions.yaml"))
        .context("加载 contradictions.yaml 失败")?;
    bundle.red_flags =
        load_list::<RedFlag>(dir.join("safety.yaml")).context("加载 safety.yaml 失败")?;
    bundle.transformations = load_list::<Transformation>(dir.join("transformations.yaml"))
        .context("加载 transformations.yaml 失败")?;
    bundle.formulas =
        load_list::<Formula>(dir.join("formulas.yaml")).context("加载 formulas.yaml 失败")?;
    bundle.cares = load_list::<CarePlan>(dir.join("care.yaml")).context("加载 care.yaml 失败")?;
    bundle.prompts =
        load_yaml::<PromptBundle>(dir.join("prompts.yaml")).context("加载 prompts.yaml 失败")?;
    bundle.routing =
        load_yaml::<Routing>(dir.join("routing.yaml")).context("加载 routing.yaml 失败")?;
    // 检索域是可选文件：老部署的 resources/ 里没有它，热重载不该因此失败，
    // 只是所有 agent 退回「不按域过滤」的宽检索。
    bundle.rag_scopes = load_yaml_opt::<RagScopes>(dir.join("rag_scopes.yaml"))
        .context("加载 rag_scopes.yaml 失败")?
        .unwrap_or_default();

    Ok(bundle)
}

fn load_yaml<T: serde::de::DeserializeOwned>(path: std::path::PathBuf) -> Result<T> {
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("找不到文件: {}", path.display()))?;
    serde_yaml::from_str(&text).with_context(|| format!("解析 YAML 失败: {}", path.display()))
}

/// 字典型 YAML（可选）：文件不存在时返回 `None` 并告警。
///
/// 用于后加入的资源文件（如 `rag_scopes.yaml`）：已有部署的 `resources/`
/// 目录里没有它，热重载不应因此失败。
fn load_yaml_opt<T: serde::de::DeserializeOwned>(path: std::path::PathBuf) -> Result<Option<T>> {
    if !path.exists() {
        tracing::warn!("可选资源缺失，按缺省处理：{}", path.display());
        return Ok(None);
    }
    load_yaml(path).map(Some)
}

/// 列表型 YAML（可选）：文件不存在时返回空列表并告警。
///
/// 用于后加入的资源文件（如 `contradictions.yaml`）：已有部署的 `resources/`
/// 目录里没有它，热重载不应因此失败——只是少了矛盾证据这一路信息。
fn load_list_opt<T: serde::de::DeserializeOwned>(path: std::path::PathBuf) -> Result<Vec<T>> {
    if !path.exists() {
        tracing::warn!("可选资源缺失，按空列表处理：{}", path.display());
        return Ok(Vec::new());
    }
    load_list(path)
}

/// 列表型 YAML：顶层为数组
fn load_list<T: serde::de::DeserializeOwned>(path: std::path::PathBuf) -> Result<Vec<T>> {
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("找不到文件: {}", path.display()))?;
    serde_yaml::from_str(&text)
        .with_context(|| format!("解析 YAML 失败（应为数组）: {}", path.display()))
}

impl ResourceBundle {
    /// 按 slug 查证候
    pub fn syndrome(&self, slug: &str) -> Option<&Syndrome> {
        self.syndromes.iter().find(|s| s.slug == slug)
    }
    /// 按 slug 查问题
    pub fn question(&self, slug: &str) -> Option<&QuestionItem> {
        self.questions.iter().find(|q| q.slug == slug)
    }
    /// 按 slug 查红色警戒
    pub fn red_flag(&self, slug: &str) -> Option<&RedFlag> {
        self.red_flags.iter().find(|r| r.slug == slug)
    }
    /// 按 slug 查方剂
    pub fn formula(&self, slug: &str) -> Option<&Formula> {
        self.formulas.iter().find(|f| f.slug == slug)
    }
}
