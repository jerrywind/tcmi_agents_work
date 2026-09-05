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

    // ---- 症状分两级（H2）：主症是定证的必要条件，次症只是旁证 ----
    //
    // 中医辨证讲「主症必备」：次症凑得再多，缺了主症也不该定证。
    // 不分级的后果是「乏力」「纳呆」「失眠」这类跨证候的非特异表现
    // 与「恶寒重发热轻」「脉浮紧」这类强特异表现同权计数，
    // 于是**症状表越长的证候越容易赢**——库外的证候（如肾阳虚）会被判成
    // 库内最像的那一个，而且看起来有置信度。见 H3。
    /// 主症：辨证的主依据，命中任一条即满足「主症必备」
    #[serde(default)]
    key_symptoms: Vec<String>,
    /// 次症：佐证，权重低于主症
    #[serde(default)]
    minor_symptoms: Vec<String>,
    /// 旧格式遗留：未分级的症状表（`symptoms:`）。
    /// 加载时并入次症，之后不再参与打分——保住老 YAML 不报错。
    #[serde(default, rename = "symptoms")]
    legacy_symptoms: Vec<String>,

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

impl Syndrome {
    /// 加载后归一化：把旧格式的 `symptoms` 并入次症。
    ///
    /// 在 `load()` 里统一做一次，之后所有消费方只读 `key_symptoms()` /
    /// `minor_symptoms()`，不必各自处理两种格式。
    pub(crate) fn normalize(&mut self) {
        if self.legacy_symptoms.is_empty() {
            return;
        }
        for s in self.legacy_symptoms.drain(..) {
            if !self.minor_symptoms.contains(&s) && !self.key_symptoms.contains(&s) {
                self.minor_symptoms.push(s);
            }
        }
    }

    /// 主症（定证的必要条件）
    pub fn key_symptoms(&self) -> &[String] {
        &self.key_symptoms
    }

    /// 次症（含旧格式 `symptoms` 归并进来的部分）
    pub fn minor_symptoms(&self) -> &[String] {
        &self.minor_symptoms
    }

    /// 全部症状：主症在前、次症在后，去重保序。
    ///
    /// 供**追问生成**使用（鉴别追问取主证与次证的症状差集）——
    /// 那里问的是「还有没有这个表现」，不区分主次。
    pub fn all_symptoms(&self) -> Vec<&String> {
        let mut out: Vec<&String> = self.key_symptoms.iter().collect();
        for s in &self.minor_symptoms {
            if !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }
}

// ------------------------- 问诊问题库 -------------------------

/// 患者性别（由 `payload.gender` 传入，前端患者档案里已采集）
///
/// 用于过滤人群专属的问诊条目：`payload.gender=男` 时仍追问月经，
/// 是人工验收点名的问题——规则层的【建议追问】会把月经列进去，
/// 模型照着列自然也就照着问。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gender {
    Male,
    Female,
    /// 未采集或无法识别。此时**不过滤**任何问题：宁可多问一句，
    /// 也不要因为性别未知就漏掉妇科相关的重要鉴别线索。
    Unknown,
}

impl Gender {
    /// 从 `payload.gender` 解析（兼容中文与英文写法）
    pub fn from_payload(payload: &serde_json::Value) -> Self {
        let raw = payload
            .get("gender")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        match raw {
            "男" | "男性" | "male" | "m" => Gender::Male,
            "女" | "女性" | "female" | "f" => Gender::Female,
            _ => Gender::Unknown,
        }
    }

    /// 中文名（用于给模型的患者画像提示）
    pub fn label(&self) -> &'static str {
        match self {
            Gender::Male => "男",
            Gender::Female => "女",
            Gender::Unknown => "未采集",
        }
    }

    /// 该人群限定取值是否适用于本患者
    ///
    /// `applies_to` 取 `any`（缺省）/ `male` / `female`。
    /// 取到不认识的值时不过滤——配错了只该是「没生效」，
    /// 不该变成「把问题全砍光」。
    ///
    /// **性别未知时按「不排除」处理**：患者档案里的性别是可选项，
    /// 把「未采集」当成「非女性」是有偏的，会系统性漏掉妇科鉴别线索。
    /// 宁可多问一句，也不要替患者先做排除。
    pub fn matches(&self, applies_to: Option<&str>) -> bool {
        match applies_to.map(|s| s.trim().to_lowercase()).as_deref() {
            None | Some("") | Some("any") | Some("all") => true,
            Some("female") => *self != Gender::Male,
            Some("male") => *self != Gender::Female,
            _ => true,
        }
    }
}

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
    /// 适用人群：`any`（缺省，所有人）/ `male` / `female`。
    ///
    /// 经带、胎产这类问题只该问女性；此前靠 prompt 里写「若是女性」
    /// 让模型自己判断，模型照抄文案后男患者也会被追问月经。
    #[serde(default)]
    pub applies_to: Option<String>,
}

impl QuestionItem {
    /// 该问题是否适用于本患者（人群维度，T7.3）
    pub fn applies_to_gender(&self, gender: Gender) -> bool {
        gender.matches(self.applies_to.as_deref())
    }
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
    /// 出处（书名）。开方步被要求「注明出处」，有据可查比让模型现编可靠。
    #[serde(default)]
    pub source: Option<String>,
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
