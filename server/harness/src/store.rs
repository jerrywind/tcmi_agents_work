//! 报告持久化（T5.1）与落盘脱敏（T5.4）
//!
//! harness 是**无状态**服务：一次 `/chat` 跑完 `routing.yaml` 全部步骤即返回，
//! 不保存会话。但「报告可回查」是刚需——用户刷新页面或换个设备后
//! 还想找回刚才那份结论，而前端 `session.ts` 只存在于内存里。
//!
//! 本模块提供一个**可选的**文件存储层，与无状态设计不冲突：
//!
//! - 未配置 `store_dir`（默认）时完全不启用，行为与此前一致；
//! - 启用后每次 `/chat` 落盘一份报告 JSON，响应里多出 `report_id`；
//! - `GET /reports/:id` 按 id 回查，`GET /reports` 列出最近若干份。
//!
//! 存的是**结果**而非会话状态：服务端依旧没有「问诊进行中」的概念，
//! 多轮仍然由调用方累积 `messages`。
//!
//! # 为什么落盘要脱敏
//! 报告里含患者自述（症状、年龄、地区、可能提到手机号/身份证）。
//! 一旦明文入库，就多了一处需要保护的个人信息。故落盘前统一走
//! [`redact_text`]，把手机号 / 身份证 / 邮箱 / 长数字串替换成占位符。
//! 脱敏只影响**存储**，不影响本次响应内容——用户看到的仍是原文。

use anyhow::{Context, Result};
use chrono::Utc;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// 报告列表默认返回条数
pub const DEFAULT_LIST_LIMIT: usize = 20;

/// 报告存储（可禁用）
#[derive(Debug, Clone)]
pub struct ReportStore {
    dir: Option<PathBuf>,
    redact: bool,
}

impl ReportStore {
    /// `dir` 为 `None` 时表示不持久化；`Some` 时确保目录存在
    pub fn new(dir: Option<PathBuf>, redact: bool) -> Result<Self> {
        if let Some(d) = &dir {
            fs::create_dir_all(d).with_context(|| format!("创建报告目录失败: {}", d.display()))?;
        }
        Ok(Self { dir, redact })
    }

    pub fn is_enabled(&self) -> bool {
        self.dir.is_some()
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// 落盘一份报告，返回其 id；未启用存储时返回 `None`
    ///
    /// `result` 为 `/chat` 的响应体，`messages` / `payload` 为本次请求入参
    /// （仅为回查时还原上下文用，落盘前脱敏）。
    pub fn save(
        &self,
        result: &Value,
        messages: &Value,
        payload: &Value,
    ) -> Result<Option<String>> {
        let Some(dir) = &self.dir else {
            return Ok(None);
        };
        let now = Utc::now();
        let id = new_report_id(now.timestamp_nanos_opt().unwrap_or_default(), result);

        let stored = json!({
            "id": id,
            "created_at": now.to_rfc3339(),
            "messages": self.maybe_redact(messages),
            "payload": self.maybe_redact(payload),
            "result": result,
        });

        // 先写临时文件再 rename：避免读取方看到一个写了一半的 JSON
        let final_path = dir.join(format!("{id}.json"));
        let tmp = dir.join(format!("{id}.json.tmp"));
        fs::write(&tmp, serde_json::to_vec_pretty(&stored)?)
            .with_context(|| format!("写入报告失败: {}", tmp.display()))?;
        fs::rename(&tmp, &final_path)
            .with_context(|| format!("报告落盘失败: {}", final_path.display()))?;
        Ok(Some(id))
    }

    /// 按 id 回查；不存在返回 `Ok(None)`
    pub fn get(&self, id: &str) -> Result<Option<Value>> {
        let Some(dir) = &self.dir else {
            return Ok(None);
        };
        if !is_safe_id(id) {
            return Ok(None);
        }
        let p = dir.join(format!("{id}.json"));
        if !p.exists() {
            return Ok(None);
        }
        let text =
            fs::read_to_string(&p).with_context(|| format!("读取报告失败: {}", p.display()))?;
        let v: Value = serde_json::from_str(&text)?;
        Ok(Some(v))
    }

    /// 列出最近的报告（按修改时间倒序），返回精简元信息
    ///
    /// 排序只依赖文件 mtime（不需要读内容），因此目录很大也不会拖慢列表；
    /// 只有进入 `limit` 之内的文件才会被真正解析。
    pub fn list(&self, limit: usize) -> Result<Vec<Value>> {
        let Some(dir) = &self.dir else {
            return Ok(Vec::new());
        };
        let mut entries: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
        for e in
            fs::read_dir(dir).with_context(|| format!("读取报告目录失败: {}", dir.display()))?
        {
            let e = e?;
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let mtime = e.metadata()?.modified().unwrap_or(std::time::UNIX_EPOCH);
            entries.push((mtime, p));
        }
        // 按 mtime 倒序；同一时刻写入的（文件系统时间戳粒度可能只到秒）
        // 再按文件名倒序兜底，保证列表顺序稳定可复现。
        entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));

        let mut out = Vec::new();
        for (_, p) in entries.into_iter().take(limit) {
            let text = match fs::read_to_string(&p) {
                Ok(t) => t,
                Err(_) => continue, // 并发写/损坏文件：跳过而不让整个列表失败
            };
            let v: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let result = v.get("result").cloned().unwrap_or(json!(null));
            out.push(json!({
                "id": v.get("id").cloned().unwrap_or_else(|| json!(p.file_stem()
                    .and_then(|s| s.to_str()).unwrap_or(""))),
                "created_at": v.get("created_at").cloned().unwrap_or(json!(null)),
                "partial": result.get("partial").cloned().unwrap_or(json!(false)),
                "blocked": result.get("blocked").cloned().unwrap_or(json!(false)),
                "steps": result.get("steps").and_then(|s| s.as_array()).map(|a| a.len()).unwrap_or(0),
                "primary_syndrome": result
                    .pointer("/structured/differentiation/primary/name")
                    .cloned()
                    .unwrap_or(json!(null)),
            }));
        }
        Ok(out)
    }

    fn maybe_redact(&self, v: &Value) -> Value {
        if self.redact {
            redact_value(v)
        } else {
            v.clone()
        }
    }
}

/// 报告 id：`<日期>-<时间>-<6 位内容散列>`
///
/// 时间前缀让目录按时间自然有序；散列后缀避免同一秒内两次请求相互覆盖
/// （即便内容相同，时间戳的纳秒部分也参与散列）。
fn new_report_id(seed: i64, content: &Value) -> String {
    let mut h = DefaultHasher::new();
    seed.hash(&mut h);
    content.to_string().hash(&mut h);
    let suffix = format!("{:06x}", h.finish() % 0x1_000_000);
    format!("{}-{suffix}", Utc::now().format("%Y%m%d-%H%M%S"))
}

/// id 只允许安全字符：直接拼进文件路径，必须挡住 `../` 之类的穿越
fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn phone_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"1[3-9]\d{9}").expect("手机号正则"))
}
fn id_card_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\d{17}[\dXx]").expect("身份证正则"))
}
fn email_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[\w.+-]+@[\w-]+\.[\w.-]+").expect("邮箱正则"))
}
fn long_digits_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\d{12,}").expect("长数字正则"))
}

/// 文本脱敏：把个人标识信息替换为占位符
///
/// 只识别**强标识**（手机号 / 身份证 / 邮箱 / 长数字串）：
/// 症状描述里的「34 岁」「血压 130」这类短数字与临床信息必须保留，
/// 否则报告就失去回查价值。
pub fn redact_text(s: &str) -> String {
    let s = phone_re().replace_all(s, "[手机号已脱敏]");
    let s = id_card_re().replace_all(&s, "[身份证号已脱敏]");
    let s = email_re().replace_all(&s, "[邮箱已脱敏]");
    long_digits_re()
        .replace_all(&s, "[长数字已脱敏]")
        .into_owned()
}

/// 递归脱敏：遍历 JSON 里的每个字符串
fn redact_value(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(redact_text(s)),
        Value::Array(a) => Value::Array(a.iter().map(redact_value).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, val)| (k.clone(), redact_value(val)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path) -> ReportStore {
        ReportStore::new(Some(dir.to_path_buf()), true).expect("store 创建失败")
    }

    #[test]
    fn disabled_store_is_a_no_op() {
        let s = ReportStore::new(None, true).unwrap();
        assert!(!s.is_enabled());
        assert_eq!(s.save(&json!({}), &json!([]), &json!({})).unwrap(), None);
        assert_eq!(s.get("x").unwrap(), None);
        assert!(s.list(10).unwrap().is_empty());
    }

    #[test]
    fn save_then_get_round_trips() {
        let dir = std::env::temp_dir().join(format!(
            "harness-store-{}",
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let s = store(&dir);
        let result = json!({"steps": [{"capability": "safety"}], "summary": "ok"});
        let id = s
            .save(
                &result,
                &json!([{"role": "user", "content": "头疼"}]),
                &json!({"age": 34}),
            )
            .unwrap()
            .expect("应返回报告 id");

        let got = s.get(&id).unwrap().expect("报告应可回查");
        assert_eq!(got["id"], json!(id));
        assert_eq!(got["result"]["summary"], json!("ok"));
        assert_eq!(got["messages"][0]["content"], json!("头疼"));
        assert_eq!(got["payload"]["age"], json!(34));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_returns_newest_first_with_meta() {
        let dir = std::env::temp_dir().join(format!(
            "harness-list-{}",
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let s = store(&dir);
        for i in 0..3 {
            s.save(
                &json!({"steps": [], "partial": i == 1, "blocked": false}),
                &json!([]),
                &json!({}),
            )
            .unwrap();
            // 文件系统时间戳粒度可能到秒：错开写入，让「最新在前」可被观测
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let items = s.list(10).unwrap();
        assert_eq!(items.len(), 3);
        assert!(
            items.iter().any(|it| it["partial"] == json!(true)),
            "partial 标记应体现在列表里：{items:?}"
        );
        assert!(items[0]["created_at"].is_string());
        assert_eq!(s.list(2).unwrap().len(), 2, "limit 应生效");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn redaction_masks_identifiers_but_keeps_clinical_numbers() {
        let raw = "患者张三，手机 13812345678，身份证 11010119900307123X，邮箱 a.b@example.com，卡号 6222021234567890123；血压 130/85，年龄 34 岁";
        let out = redact_text(raw);
        assert!(!out.contains("13812345678"), "{out}");
        assert!(!out.contains("11010119900307123X"), "{out}");
        assert!(!out.contains("a.b@example.com"), "{out}");
        assert!(!out.contains("6222021234567890123"), "{out}");
        // 临床信息必须保留，否则报告失去回查价值
        assert!(out.contains("130/85"), "{out}");
        assert!(out.contains("34 岁"), "{out}");
    }

    #[test]
    fn store_redacts_messages_before_writing() {
        let dir = std::env::temp_dir().join(format!(
            "harness-redact-{}",
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let s = store(&dir);
        let msgs = json!([{"role": "user", "content": "我叫张三，手机 13812345678，最近口苦"}]);
        let id = s
            .save(&json!({"summary": "x"}), &msgs, &json!({}))
            .unwrap()
            .unwrap();
        let got = s.get(&id).unwrap().unwrap();
        assert!(got["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("[手机号已脱敏]"));

        // 关闭脱敏后应原样落盘（运维明确要求时可用）
        let s2 = ReportStore::new(Some(dir.clone()), false).unwrap();
        let id2 = s2
            .save(&json!({"summary": "y"}), &msgs, &json!({}))
            .unwrap()
            .unwrap();
        let got2 = s2.get(&id2).unwrap().unwrap();
        assert!(got2["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("13812345678"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_traversal_ids_are_rejected() {
        let dir = std::env::temp_dir().join(format!(
            "harness-trav-{}",
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let s = store(&dir);
        for bad in ["../secret", "a/b", "", "..\\x"] {
            assert_eq!(s.get(bad).unwrap(), None, "{bad} 应被拒绝");
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
