//! 资源数据模型（对应 backend `knowledge/*.py` 中的常量与 `app/agents/*` 的提示词）
//!
//! 所有结构体均从 `resources/*.yaml` 反序列化得到。
//! 设计原则：
//! - 字段 key 用稳定英文 slug（如 `tongue_body`），便于程序索引；
//! - 字段值为中文文案，中医专业人士可直接修改；
//! - 每个文件顶部有中文注释说明用途。

use serde::Deserialize;
use std::collections::BTreeMap;

/// 整个资源包：加载后常驻内存
#[derive(Debug, Clone, Default)]
pub struct ResourceBundle {
    pub syndromes: Vec<Syndrome>,
    pub questions: Vec<QuestionItem>,
    pub keyword_evidence: Vec<KeywordEvidence>,
    pub contradictions: Vec<Contradiction>,
    pub red_flags: Vec<RedFlag>,
    pub transformations: Vec<Transformation>,
    pub formulas: Vec<Formula>,
    pub cares: Vec<CarePlan>,
    pub prompts: PromptBundle,
    pub routing: Routing,
    /// capability slug -> 典籍检索域（见 `resources/rag_scopes.yaml`）
    pub rag_scopes: RagScopes,
}

// ------------------------- 证候库 -------------------------
#[derive(Debug, Clone, Deserialize)]
pub struct Syndrome {
    pub slug: String, // 英文 slug，如 wind_cold_attack_lung
    pub name: String, // 中文名，如 风寒袭肺证
    #[serde(default)]
    pub meridian: Option<String>, // 涉及经络/脏腑
    #[serde(default)]
    pub symptoms: Vec<String>, // 典型症状
    #[serde(default)]
    pub tongue: Option<String>, // 舌象
    #[serde(default)]
    pub pulse: Option<String>, // 脉象
    #[serde(default)]
    pub pathogenesis: Option<String>, // 病机
    /// 治则（立法依据）。立法 agent 据此给出确定性的治则，
    /// 避免模型凭空起治法名（如把「辛温解表」说成「发散风寒」这类自造词）。
    #[serde(default)]
    pub principles: Vec<String>,
    /// 相关临床学科（与典籍分类的 `临床学科` 维度同名标签）。
    ///
    /// 辨证完成后写入共享状态，`tcm-rag` 据此把检索范围收窄到该科室——
    /// 辨证出儿科，开方就只看儿科方书。
    #[serde(default)]
    pub departments: Vec<String>,
}

// ------------------------- 问诊问题库 -------------------------
#[derive(Debug, Clone, Deserialize)]
pub struct QuestionItem {
    pub slug: String,   // 英文 slug，如 fever
    pub prompt: String, // 向医生提问的中文文案
    #[serde(default)]
    pub category: Option<String>, // 分组：寒热/汗出/头身/二便...
    #[serde(default)]
    pub evidence_keys: Vec<String>, // 命中后关联的证据 key
    #[serde(default)]
    pub priority: u8, // 优先级（越小越先问）
    /// 该信息该由哪个采集 agent 负责（inspection / listening / inquiry / palpation）。
    ///
    /// 反馈式辨证 loop 在**第二轮及以后**据此只跑必要的采集 agent：
    /// 首轮四诊全跑，后续轮若只剩「舌苔什么颜色」，就只跑望诊。
    #[serde(default)]
    pub agent: Option<String>,
}

// ------------------------- 关键词证据映射 -------------------------
#[derive(Debug, Clone, Deserialize)]
pub struct KeywordEvidence {
    pub slug: String,  // 证据 key，如 wind_cold
    pub label: String, // 中文标签，如 风寒
    #[serde(default)]
    pub keywords: Vec<String>, // 触发关键词
    #[serde(default)]
    pub syndromes: Vec<String>, // 指向的证候 slug
    #[serde(default)]
    pub note: Option<String>, // 说明
}

// ------------------------- 红色警戒（安全门） -------------------------
#[derive(Debug, Clone, Deserialize)]
pub struct RedFlag {
    pub slug: String,  // 英文 slug，如 chest_pain
    pub label: String, // 中文标签，如 胸痛
    #[serde(default)]
    pub keywords: Vec<String>, // 触发关键词
    #[serde(default)]
    pub advice: String, // 给用户的警示文案
    #[serde(default)]
    pub severity: String, // low | medium | high | critical
}

// ------------------------- 相反表现（矛盾证据，T4.1） -------------------------
#[derive(Debug, Clone, Deserialize)]
pub struct Contradiction {
    pub slug: String, // 英文 slug，如 sweat
    pub a: String,    // 互斥表现之一，如 无汗
    pub b: String,    // 互斥表现之二，如 有汗
    #[serde(default)]
    pub note: Option<String>, // 说明
}

impl Contradiction {
    /// 给定已命中的表现 `term`，返回与之矛盾**且确实出现在语料中**的表现。
    ///
    /// 判定双向：命中 a 且语料出现 b，或命中 b 且语料出现 a。
    /// 语料里没有相反表现时返回 `None`——没有出现过的表现不构成矛盾证据。
    pub fn opposite_in<'a>(&'a self, term: &str, text: &str) -> Option<&'a str> {
        if term == self.a && text.contains(self.b.as_str()) {
            Some(self.b.as_str())
        } else if term == self.b && text.contains(self.a.as_str()) {
            Some(self.a.as_str())
        } else {
            None
        }
    }
}

// ------------------------- 传变（疾病发展） -------------------------
#[derive(Debug, Clone, Deserialize)]
pub struct Transformation {
    pub slug: String,  // 英文 slug
    pub from: String,  // 来源证候 slug
    pub to: String,    // 目标证候 slug
    pub label: String, // 中文描述
    #[serde(default)]
    pub probability: Option<String>, // 概率/条件说明
}

// ------------------------- 方剂库 -------------------------
#[derive(Debug, Clone, Deserialize)]
pub struct Formula {
    pub slug: String, // 英文 slug，如 ma_xing_gan_shi
    pub name: String, // 中文名，如 麻杏甘石汤
    #[serde(default)]
    pub for_syndromes: Vec<String>, // 适用证候 slug
    #[serde(default)]
    pub composition: Vec<String>, // 组成（药名）
    #[serde(default)]
    pub usage: Option<String>, // 用法
    #[serde(default)]
    pub caution: Option<String>, // 禁忌/注意
}

// ------------------------- 调护方案 -------------------------
#[derive(Debug, Clone, Deserialize)]
pub struct CarePlan {
    pub slug: String,  // 英文 slug，如 wind_cold_care
    pub label: String, // 中文标签
    #[serde(default)]
    pub for_syndromes: Vec<String>, // 适用证候 slug
    #[serde(default)]
    pub items: Vec<String>, // 调护条目（饮食/起居/情志）
}

// ------------------------- 提示词包 -------------------------
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PromptBundle {
    #[serde(default)]
    pub system: String, // 总系统提示
    #[serde(default)]
    pub inspection: String,
    #[serde(default)]
    pub listening: String,
    #[serde(default)]
    pub inquiry: String,
    #[serde(default)]
    pub palpation: String,
    #[serde(default)]
    pub differentiation: String,
    #[serde(default)]
    pub safety: String,
    #[serde(default)]
    pub treatment: String,
    // ---- 治疗期拆分出的专职 agent ----
    #[serde(default)]
    pub case_reference: String,
    #[serde(default)]
    pub strategy: String,
    #[serde(default)]
    pub herbology: String,
    #[serde(default)]
    pub prescription: String,
    #[serde(default)]
    pub care: String,
    #[serde(default)]
    pub acupuncture: String,
}

// ------------------------- 典籍检索域 -------------------------
/// 各 sub-agent 的典籍检索域。
///
/// 694 部典籍不该被任何单个 agent 全看——切诊翻《脉经》、开方翻《普济方》，
/// 混着检索只会互相稀释。这里用四维分类标签给每个 agent 圈定范围。
pub type RagScopes = BTreeMap<String, RagScope>;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RagScope {
    /// 内容体裁（静态，agent 固有）
    #[serde(default)]
    pub genres: Vec<String>,
    /// 功能用途（静态，体裁的细分）
    #[serde(default)]
    pub functions: Vec<String>,
    /// 临床学科。含 `"dynamic"` 表示**由辨证结果动态注入**
    /// （辨证出儿科，开方就只看儿科方书）。
    #[serde(default)]
    pub departments: Vec<String>,
    /// 学术流派。留空 = 不过滤（避免学术偏见）；
    /// 仅当请求 `payload.school` 指定时才注入。
    #[serde(default)]
    pub schools: Vec<String>,
    #[serde(default)]
    pub top_k: Option<u32>,
}

impl RagScope {
    /// 科室是否由辨证结果动态注入
    pub fn dynamic_department(&self) -> bool {
        self.departments.iter().any(|d| d == "dynamic")
    }
}

// ------------------------- 路由（当前激活的 agent） -------------------------
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Routing {
    #[serde(default)]
    pub active: Vec<String>, // 激活的 capability slug 列表，按问诊顺序
    #[serde(default)]
    pub default: Option<String>, // 默认入口 capability
    /// 命名档位：`compatible`（7 步）/ `standard`（10 步）/ `full`（12 步）
    #[serde(default)]
    pub profiles: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub active_profile: Option<String>,
}
