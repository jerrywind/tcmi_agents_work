//! rrserver CLI：云端中继服务器（`server`）与家庭端隧道客户端（`client`）。

use anyhow::Context;
use clap::{Parser, Subcommand};
use std::sync::Arc;
use std::time::Duration;
use toml::Value;
use tracing::info;

use rrserver::skill::{ConstState, JudgeEngine, SkillRule, SkillSet};
use rrserver::{client, llmsrv, server, state};

use server::{AppState, HealthConfig, TunnelAuth};

#[derive(Parser)]
#[command(
    name = "rrserver",
    about = "Reverse relay server for home LLM tunneling"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 云端中继服务器（通常被 nginx 反代，仅监听内网）
    Server {
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: String,
        #[arg(long, help = "TOML 配置文件路径，含 [[tunnels]] 与 external_ws_base")]
        config: Option<String>,
        #[arg(long, default_value = "wss://rr.example.com/rr")]
        external_ws_base: String,
    },
    /// 家庭端隧道客户端（主动注册到云端，转发本地 llm 服务）
    Client {
        #[arg(long, help = "云端 register 基址，如 https://rr.example.com/rr")]
        server: String,
        #[arg(long, help = "隧道名（与云端配置一致）")]
        name: String,
        #[arg(long, help = "隧道 token（与云端配置一致）")]
        token: String,
        #[arg(
            long,
            default_value = "http://127.0.0.1:8080",
            help = "本地 llm 服务基址"
        )]
        local: String,
    },
    /// 模型部署包装：启动/接入本地模型服务，并注册到 rrserver 隧道
    LlmServer {
        #[arg(long, help = "部署配置 TOML 路径")]
        config: String,
        #[arg(long, help = "覆盖：云端 register 基址")]
        server: Option<String>,
        #[arg(long, help = "覆盖：隧道名")]
        name: Option<String>,
        #[arg(long, help = "覆盖：隧道 token")]
        token: Option<String>,
    },
}

/// `load_config` 的返回：隧道凭据、external_ws_base、可选技能集、心跳/探活配置。
struct ServerConfig {
    tunnels: Vec<(String, String)>,
    external_ws_base: String,
    skills: Option<Arc<SkillSet>>,
    health: HealthConfig,
}

/// 读取 `[health]` 表：全部为可选秒级配置，缺省走 [`HealthConfig::default`]。
fn load_health(doc: &toml::Value) -> HealthConfig {
    let mut h = HealthConfig::default();
    let secs = |key: &str| -> Option<u64> {
        doc.get("health")
            .and_then(|v: &Value| v.get(key))
            .and_then(|v: &Value| v.as_integer())
            .map(|i| i.max(0) as u64)
    };
    if let Some(v) = secs("heartbeat_interval_secs") {
        h.heartbeat_interval = Duration::from_secs(v);
    }
    if let Some(v) = secs("heartbeat_timeout_secs") {
        h.heartbeat_timeout = Duration::from_secs(v);
    }
    if let Some(v) = secs("probe_timeout_secs") {
        h.probe_timeout = Duration::from_secs(v);
    }
    if let Some(v) = secs("first_response_timeout_secs") {
        h.first_response_timeout = Duration::from_secs(v);
    }
    if let Some(v) = secs("request_timeout_secs") {
        h.request_timeout = Duration::from_secs(v);
    }
    if let Some(v) = secs("reaper_interval_secs") {
        h.reaper_interval = Duration::from_secs(v);
    }
    h
}

fn load_config(path: Option<&str>) -> anyhow::Result<ServerConfig> {
    let path = match path {
        Some(p) => p,
        None => {
            return Ok(ServerConfig {
                tunnels: vec![],
                external_ws_base: String::new(),
                skills: None,
                health: HealthConfig::default(),
            })
        }
    };
    let content = std::fs::read_to_string(path).with_context(|| format!("read config {}", path))?;
    let doc: toml::Value = toml::from_str(&content).context("parse config")?;
    let mut tokens = vec![];
    if let Some(arr) = doc.get("tunnels").and_then(|v: &Value| v.as_array()) {
        for t in arr {
            let name = t
                .get("name")
                .and_then(|v: &Value| v.as_str())
                .unwrap_or("")
                .to_string();
            let token = t
                .get("token")
                .and_then(|v: &Value| v.as_str())
                .unwrap_or("")
                .to_string();
            if !name.is_empty() {
                tokens.push((name, token));
            }
        }
    }
    let ws_base = doc
        .get("external_ws_base")
        .and_then(|v: &Value| v.as_str())
        .unwrap_or("")
        .to_string();

    // 可选技能闸门：配置 [[skills]] 后启用；带 X-Skill 头的请求会被冷却 / 资源 / 状态校验。
    // 所有技能共享同一全局预算（skill_budget），每次放行的请求按各自 cost 扣减。
    let skills = doc
        .get("skills")
        .and_then(|v: &Value| v.as_array())
        .filter(|a| !a.is_empty())
        .map(|arr| {
            let budget = doc
                .get("skill_budget")
                .and_then(|v: &Value| v.as_integer())
                .unwrap_or(1_000_000) as u64;
            let engine = Arc::new(JudgeEngine::new(
                budget,
                Arc::new(ConstState("idle".into())),
            ));
            let set = Arc::new(SkillSet::new(engine));
            for s in arr {
                let name = s
                    .get("name")
                    .and_then(|v: &Value| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                let cooldown_secs = s
                    .get("cooldown_secs")
                    .and_then(|v: &Value| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                    .unwrap_or(0.0);
                let cost = s
                    .get("cost")
                    .and_then(|v: &Value| v.as_integer())
                    .unwrap_or(0) as u64;
                let required_state = s
                    .get("required_state")
                    .and_then(|v: &Value| v.as_str())
                    .map(|s| s.to_string());
                set.register(SkillRule {
                    name,
                    cooldown: Duration::from_secs_f64(cooldown_secs.max(0.0)),
                    cost,
                    required_state,
                });
            }
            set
        });

    Ok(ServerConfig {
        tunnels: tokens,
        external_ws_base: ws_base,
        skills,
        health: load_health(&doc),
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Server {
            listen,
            config,
            external_ws_base,
        } => {
            let cfg = load_config(config.as_deref())?;
            if cfg.tunnels.is_empty() {
                anyhow::bail!("no tunnels configured; add [[tunnels]] to config");
            }
            let ws_base = if cfg.external_ws_base.is_empty() {
                external_ws_base
            } else {
                cfg.external_ws_base
            };
            let auth = TunnelAuth::from_list(&cfg.tunnels);
            info!(
                "health config: heartbeat_interval={}s heartbeat_timeout={}s probe_timeout={}s first_response_timeout={}s request_timeout={}s reaper_interval={}s",
                cfg.health.heartbeat_interval.as_secs(),
                cfg.health.heartbeat_timeout.as_secs(),
                cfg.health.probe_timeout.as_secs(),
                cfg.health.first_response_timeout.as_secs(),
                cfg.health.request_timeout.as_secs(),
                cfg.health.reaper_interval.as_secs(),
            );
            let state = AppState {
                registry: state::Registry::new(),
                services: rrserver::registry::ServiceRegistry::new(),
                auth,
                external_ws_base: ws_base,
                skills: cfg.skills,
                health: cfg.health,
                http: reqwest::Client::new(),
            };
            server::run_server(listen, state).await?;
        }
        Command::Client {
            server,
            name,
            token,
            local,
        } => {
            let cfg = client::ClientConfig {
                server_base: server,
                name,
                token,
                local_url: local,
            };
            client::run_client(cfg).await?;
        }
        Command::LlmServer {
            config,
            server,
            name,
            token,
        } => {
            let mut cfg = llmsrv::DeploymentConfig::from_toml(&config)?;
            cfg.apply_overrides(server, name, token);
            llmsrv::run(cfg).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(path: &str, content: &str) {
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn load_config_parses_tunnels_and_ws_base() {
        let path = std::env::temp_dir().join(format!("rrsrv_main_{}.toml", std::process::id()));
        let p = path.to_str().unwrap();
        write_temp(
            p,
            r#"
external_ws_base = "wss://rr.example.com/rr"

[[tunnels]]
name = "home"
token = "s3cr3t"

[[tunnels]]
name = "other"
token = "othertok"
"#,
        );
        let cfg = load_config(Some(p)).unwrap();
        std::fs::remove_file(p).ok();
        assert_eq!(cfg.tunnels.len(), 2);
        assert_eq!(cfg.tunnels[0], ("home".to_string(), "s3cr3t".to_string()));
        assert_eq!(
            cfg.tunnels[1],
            ("other".to_string(), "othertok".to_string())
        );
        assert_eq!(cfg.external_ws_base, "wss://rr.example.com/rr");
        assert!(cfg.skills.is_none());
        // 未配置 [health] 时走默认：30 分钟心跳 / 40 分钟阈值 / 1 分钟探活
        assert_eq!(cfg.health.heartbeat_interval, Duration::from_secs(1800));
        assert_eq!(cfg.health.heartbeat_timeout, Duration::from_secs(2400));
        assert_eq!(cfg.health.probe_timeout, Duration::from_secs(60));
        assert_eq!(cfg.health.first_response_timeout, Duration::from_secs(60));
        assert_eq!(cfg.health.reaper_interval, Duration::from_secs(60));
    }

    #[test]
    fn load_config_parses_health_section() {
        let path =
            std::env::temp_dir().join(format!("rrsrv_main_health_{}.toml", std::process::id()));
        let p = path.to_str().unwrap();
        write_temp(
            p,
            r#"
[[tunnels]]
name = "home"
token = "s3cr3t"

[health]
heartbeat_interval_secs = 30
heartbeat_timeout_secs = 40
probe_timeout_secs = 1
first_response_timeout_secs = 2
request_timeout_secs = 30
reaper_interval_secs = 5
"#,
        );
        let cfg = load_config(Some(p)).unwrap();
        std::fs::remove_file(p).ok();
        assert_eq!(cfg.health.heartbeat_interval, Duration::from_secs(30));
        assert_eq!(cfg.health.heartbeat_timeout, Duration::from_secs(40));
        assert_eq!(cfg.health.probe_timeout, Duration::from_secs(1));
        assert_eq!(cfg.health.first_response_timeout, Duration::from_secs(2));
        assert_eq!(cfg.health.request_timeout, Duration::from_secs(30));
        assert_eq!(cfg.health.reaper_interval, Duration::from_secs(5));
    }

    #[test]
    fn load_config_parses_skills_section() {
        let path = std::env::temp_dir().join(format!("rrsrv_main_sk_{}.toml", std::process::id()));
        let p = path.to_str().unwrap();
        write_temp(
            p,
            r#"
external_ws_base = "wss://rr.example.com/rr"
skill_budget = 500

[[tunnels]]
name = "home"
token = "s3cr3t"

[[skills]]
name = "fire"
cooldown_secs = 30
cost = 2
"#,
        );
        let cfg = load_config(Some(p)).unwrap();
        std::fs::remove_file(p).ok();
        assert_eq!(cfg.tunnels.len(), 1);
        let set = cfg.skills.expect("skills should be enabled");
        assert!(set.contains("fire"));
        let rule = set.get("fire").unwrap();
        assert_eq!(rule.cooldown, Duration::from_secs(30));
        assert_eq!(rule.cost, 2);
    }

    #[test]
    fn load_config_missing_path_returns_empty() {
        let cfg = load_config(None).unwrap();
        assert!(cfg.tunnels.is_empty());
        assert!(cfg.external_ws_base.is_empty());
        assert!(cfg.skills.is_none());
        assert_eq!(cfg.health.heartbeat_timeout, Duration::from_secs(2400));
    }
}
