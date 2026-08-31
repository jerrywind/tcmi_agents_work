//! llm_server 侧服务的注册中心：hash code 签发、心跳记录与过期扫描。
//!
//! 每个由 llm_server 主动注册上来的服务都会拿到一个**独立 hash code**，
//! 后续心跳上报、注销、探活都以它为凭证（不再重复携带 name + token）。
//!
//! 心跳契约（默认值见 [`crate::server::HealthConfig`]）：
//! - 服务每 30 分钟主动上报一次心跳；
//! - 云端每 60 秒扫描一次，找出 40 分钟没有心跳的服务；
//! - 对这些服务主动发起探活，1 分钟内没有回应或回应异常，则记录日志并注销注册。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tokio::sync::Mutex;

/// 注册服务的接入形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// WebSocket 反向隧道：服务位于 NAT 之后，主动外连云端（默认）。
    Ws,
    /// HTTP 直连：云端可直接访问服务注册的 `endpoint`。
    Http,
}

impl Transport {
    pub fn as_str(&self) -> &'static str {
        match self {
            Transport::Ws => "ws",
            Transport::Http => "http",
        }
    }

    /// 解析注册请求中的 `transport` 字段；未知取值返回 `None`。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ws" | "websocket" | "tunnel" => Some(Transport::Ws),
            "http" | "https" => Some(Transport::Http),
            _ => None,
        }
    }
}

/// 一条服务注册记录。
#[derive(Debug, Clone)]
pub struct Registration {
    /// 本次注册签发的独立 hash code（唯一凭证）。
    pub hash: String,
    /// 隧道 / 服务名（与云端 `[[tunnels]]` 中的 name 一致）。
    pub name: String,
    /// 接入形态。
    pub transport: Transport,
    /// HTTP 直连形态下，服务自身可被云端访问的基址。
    pub endpoint: Option<String>,
    pub registered_at: Instant,
    /// 最近一次收到心跳的时间。
    pub last_heartbeat: Instant,
    /// 最近一次**探活成功**的时间。
    ///
    /// 探活成功说明服务确实在运行，此时即使心跳缺失也不注销注册；
    /// 同时用它作为「已探活」的记号，避免回收任务每轮重复探活、刷爆日志。
    pub last_probe: Option<Instant>,
    /// 约定给该服务的心跳周期。
    pub heartbeat_interval: Duration,
}

impl Registration {
    /// 记录一次心跳上报。
    pub fn note_heartbeat(&mut self) {
        self.last_heartbeat = Instant::now();
    }

    /// 记录一次成功的探活。
    pub fn note_probe(&mut self) {
        self.last_probe = Some(Instant::now());
    }

    /// 距最近一次「活跃证据」（心跳或成功探活）过去了多久。
    pub fn silence(&self) -> Duration {
        let mut latest = self.last_heartbeat;
        if let Some(p) = self.last_probe {
            if p > latest {
                latest = p;
            }
        }
        latest.elapsed()
    }

    /// 静默时长是否超过给定阈值（即：该被探活 / 回收了）。
    pub fn is_stale(&self, timeout: Duration) -> bool {
        self.silence() > timeout
    }

    /// 距最近一次心跳过去了多久（不受探活影响，用于展示与诊断）。
    pub fn heartbeat_age(&self) -> Duration {
        self.last_heartbeat.elapsed()
    }

    /// 序列化为 `/api/services` 的展示结构。
    pub fn to_json(&self, stale: bool) -> Value {
        json!({
            "name": self.name,
            "hash": self.hash,
            "transport": self.transport.as_str(),
            "endpoint": self.endpoint,
            "registered_secs_ago": self.registered_at.elapsed().as_secs(),
            "heartbeat_age_secs": self.heartbeat_age().as_secs(),
            "silence_secs": self.silence().as_secs(),
            // 周期类字段统一毫秒（与注册 / 心跳响应一致）；已过去时长用秒便于阅读
            "heartbeat_interval_millis": self.heartbeat_interval.as_millis() as u64,
            "stale": stale,
        })
    }
}

/// 服务注册中心：进程内唯一，按 name 索引，按 hash 检索。
#[derive(Clone, Default)]
pub struct ServiceRegistry {
    inner: Arc<Mutex<HashMap<String, Registration>>>,
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册（同名覆盖，并**重新签发** hash code）。
    pub async fn register(
        &self,
        name: &str,
        transport: Transport,
        endpoint: Option<String>,
        heartbeat_interval: Duration,
    ) -> Registration {
        let reg = Registration {
            hash: new_hash_code(name),
            name: name.to_string(),
            transport,
            endpoint,
            registered_at: Instant::now(),
            last_heartbeat: Instant::now(),
            last_probe: None,
            heartbeat_interval,
        };
        self.inner
            .lock()
            .await
            .insert(name.to_string(), reg.clone());
        reg
    }

    pub async fn get(&self, name: &str) -> Option<Registration> {
        self.inner.lock().await.get(name).cloned()
    }

    /// 按 hash code 查找注册记录。
    pub async fn find_by_hash(&self, hash: &str) -> Option<Registration> {
        self.inner
            .lock()
            .await
            .values()
            .find(|r| r.hash == hash)
            .cloned()
    }

    /// 判断 `credential` 是否就是该 name 当前注册的 hash code
    /// （WS 接入允许用 hash 替代 token）。
    pub async fn matches_hash(&self, name: &str, credential: &str) -> bool {
        match self.inner.lock().await.get(name) {
            Some(r) => r.hash == credential,
            None => false,
        }
    }

    /// 记录一次心跳；返回更新后的记录（hash 不存在时为 `None`）。
    pub async fn heartbeat(&self, hash: &str) -> Option<Registration> {
        let mut map = self.inner.lock().await;
        let name = map
            .values()
            .find(|r| r.hash == hash)
            .map(|r| r.name.clone())?;
        let reg = map.get_mut(&name)?;
        reg.note_heartbeat();
        Some(reg.clone())
    }

    /// 记录一次成功探活。
    pub async fn note_probe(&self, hash: &str) {
        let mut map = self.inner.lock().await;
        if let Some(reg) = map.values_mut().find(|r| r.hash == hash) {
            reg.note_probe();
        }
    }

    /// 注销：按 hash 移除注册记录。
    pub async fn remove_by_hash(&self, hash: &str) -> Option<Registration> {
        let mut map = self.inner.lock().await;
        let name = map
            .values()
            .find(|r| r.hash == hash)
            .map(|r| r.name.clone())?;
        map.remove(&name)
    }

    pub async fn remove_by_name(&self, name: &str) -> Option<Registration> {
        self.inner.lock().await.remove(name)
    }

    pub async fn list(&self) -> Vec<Registration> {
        self.inner.lock().await.values().cloned().collect()
    }

    /// 静默时长超过 `timeout` 的注册（供心跳回收任务扫描）。
    pub async fn stale(&self, timeout: Duration) -> Vec<Registration> {
        self.inner
            .lock()
            .await
            .values()
            .filter(|r| r.is_stale(timeout))
            .cloned()
            .collect()
    }
}

/// FNV-1a 64 位哈希（零依赖，够用且可测）。
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 为一次注册签发独立的 hash code。
///
/// 输入混合了服务名、UUIDv4 与纳秒时间戳，输出 16 位小写十六进制。
/// 同名服务每次重新注册都会拿到**不同**的 hash，便于识别「重连 / 重启」。
pub fn new_hash_code(name: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_else(|_| "0".to_string());
    let mut buf = Vec::with_capacity(name.len() + 40 + nanos.len());
    buf.extend_from_slice(name.as_bytes());
    buf.push(b'|');
    buf.extend_from_slice(uuid::Uuid::new_v4().to_string().as_bytes());
    buf.push(b'|');
    buf.extend_from_slice(nanos.as_bytes());
    format!("{:016x}", fnv1a64(&buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(name: &str, hash: &str, heartbeat_ago: Duration) -> Registration {
        let now = Instant::now();
        Registration {
            hash: hash.to_string(),
            name: name.to_string(),
            transport: Transport::Ws,
            endpoint: None,
            registered_at: now - Duration::from_secs(3600),
            last_heartbeat: now - heartbeat_ago,
            last_probe: None,
            heartbeat_interval: Duration::from_secs(1800),
        }
    }

    #[test]
    fn new_hash_code_is_16_hex_chars() {
        let h = new_hash_code("home");
        assert_eq!(h.len(), 16);
        assert!(h
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
    }

    #[test]
    fn new_hash_code_is_unique_per_registration() {
        // 同名连续注册也必须拿到不同的 hash（用于识别重连）
        let a = new_hash_code("home");
        let b = new_hash_code("home");
        assert_ne!(a, b);
    }

    #[test]
    fn transport_parse_accepts_known_aliases() {
        assert_eq!(Transport::parse("ws"), Some(Transport::Ws));
        assert_eq!(Transport::parse("WebSocket"), Some(Transport::Ws));
        assert_eq!(Transport::parse("http"), Some(Transport::Http));
        assert_eq!(Transport::parse("HTTPS"), Some(Transport::Http));
        assert_eq!(Transport::parse("grpc"), None);
    }

    #[test]
    fn transport_as_str_round_trips() {
        for t in [Transport::Ws, Transport::Http] {
            assert_eq!(Transport::parse(t.as_str()), Some(t));
        }
    }

    #[test]
    fn silence_prefers_latest_evidence() {
        let mut r = reg("home", "h1", Duration::from_secs(100));
        assert_eq!(r.heartbeat_age().as_secs(), 100);
        // 探活成功比心跳更新的，静默时间应以探活为准
        r.note_probe();
        assert!(r.silence() < Duration::from_secs(1));
        assert!(r.heartbeat_age() >= Duration::from_secs(100));
    }

    #[test]
    fn is_stale_compares_silence_with_timeout() {
        let r = reg("home", "h1", Duration::from_secs(100));
        assert!(!r.is_stale(Duration::from_secs(600)));
        assert!(r.is_stale(Duration::from_secs(10)));
    }

    #[test]
    fn note_heartbeat_resets_age() {
        let mut r = reg("home", "h1", Duration::from_secs(100));
        r.note_heartbeat();
        assert!(r.heartbeat_age() < Duration::from_secs(1));
        assert!(!r.is_stale(Duration::from_secs(600)));
    }

    #[test]
    fn to_json_exposes_diagnostics() {
        let r = reg("home", "h1", Duration::from_secs(10));
        let v = r.to_json(false);
        assert_eq!(v["name"], "home");
        assert_eq!(v["hash"], "h1");
        assert_eq!(v["transport"], "ws");
        assert_eq!(v["stale"], false);
        assert!(v["heartbeat_interval_millis"].as_u64().unwrap() >= 1_800_000);
    }

    #[tokio::test]
    async fn register_issues_hash_and_overwrites_same_name() {
        let sr = ServiceRegistry::new();
        let a = sr
            .register("home", Transport::Ws, None, Duration::from_secs(1800))
            .await;
        let b = sr
            .register(
                "home",
                Transport::Http,
                Some("http://x".into()),
                Duration::from_secs(60),
            )
            .await;
        assert_ne!(a.hash, b.hash, "重注册必须换发新的 hash");
        assert_eq!(sr.list().await.len(), 1);
        let cur = sr.get("home").await.unwrap();
        assert_eq!(cur.hash, b.hash);
        assert_eq!(cur.transport, Transport::Http);
        assert_eq!(cur.endpoint.as_deref(), Some("http://x"));
    }

    #[tokio::test]
    async fn heartbeat_updates_only_matching_hash() {
        let sr = ServiceRegistry::new();
        let a = sr
            .register("home", Transport::Ws, None, Duration::from_secs(1))
            .await;
        let b = sr
            .register("other", Transport::Ws, None, Duration::from_secs(1))
            .await;
        assert!(sr.heartbeat("no-such-hash").await.is_none());
        let updated = sr.heartbeat(&a.hash).await.expect("hash 应命中");
        assert_eq!(updated.name, "home");
        assert!(updated.heartbeat_age() < Duration::from_secs(1));
        // 另一条注册不受影响
        assert_eq!(sr.heartbeat(&b.hash).await.unwrap().name, "other");
    }

    #[tokio::test]
    async fn matches_hash_and_find_by_hash() {
        let sr = ServiceRegistry::new();
        let a = sr
            .register("home", Transport::Ws, None, Duration::from_secs(1))
            .await;
        assert!(sr.matches_hash("home", &a.hash).await);
        assert!(!sr.matches_hash("home", "bogus").await);
        assert!(!sr.matches_hash("nope", &a.hash).await);
        assert_eq!(sr.find_by_hash(&a.hash).await.unwrap().name, "home");
        assert!(sr.find_by_hash("bogus").await.is_none());
    }

    #[tokio::test]
    async fn remove_by_hash_drops_registration() {
        let sr = ServiceRegistry::new();
        let a = sr
            .register("home", Transport::Ws, None, Duration::from_secs(1))
            .await;
        sr.register("other", Transport::Ws, None, Duration::from_secs(1))
            .await;
        assert!(sr.remove_by_hash("bogus").await.is_none());
        assert_eq!(sr.remove_by_hash(&a.hash).await.unwrap().name, "home");
        assert!(sr.get("home").await.is_none());
        assert_eq!(sr.list().await.len(), 1);
    }

    #[tokio::test]
    async fn stale_returns_only_silent_registrations() {
        let sr = ServiceRegistry::new();
        sr.register("fresh", Transport::Ws, None, Duration::from_secs(1))
            .await;
        let old = {
            let r = sr
                .register("old", Transport::Ws, None, Duration::from_secs(1))
                .await;
            r
        };
        // 把 old 的心跳时间往回拨 100s
        {
            let mut map = sr.inner.lock().await;
            let r = map.get_mut("old").unwrap();
            r.last_heartbeat = Instant::now() - Duration::from_secs(100);
        }
        let stale = sr.stale(Duration::from_secs(10)).await;
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].hash, old.hash);
    }

    #[tokio::test]
    async fn note_probe_suppresses_repeated_stale_hits() {
        let sr = ServiceRegistry::new();
        let r = sr
            .register("home", Transport::Ws, None, Duration::from_secs(1))
            .await;
        {
            let mut map = sr.inner.lock().await;
            let r = map.get_mut("home").unwrap();
            r.last_heartbeat = Instant::now() - Duration::from_secs(100);
        }
        assert_eq!(sr.stale(Duration::from_secs(10)).await.len(), 1);
        sr.note_probe(&r.hash).await;
        // 探活成功后，静默时间归零，不应再被扫出来（避免每轮重复探活）
        assert!(sr.stale(Duration::from_secs(10)).await.is_empty());
    }

    #[tokio::test]
    async fn registry_is_safe_under_concurrent_access() {
        let sr = Arc::new(ServiceRegistry::new());
        let mut handles = Vec::new();
        for i in 0..8u32 {
            let sr = sr.clone();
            handles.push(tokio::spawn(async move {
                let name = format!("s{}", i);
                let r = sr
                    .register(&name, Transport::Ws, None, Duration::from_secs(1))
                    .await;
                assert!(sr.heartbeat(&r.hash).await.is_some());
                assert!(sr.find_by_hash(&r.hash).await.is_some());
                sr.remove_by_hash(&r.hash).await
            }));
        }
        for h in handles {
            assert!(h.await.unwrap().is_some());
        }
        assert!(sr.list().await.is_empty());
    }
}
