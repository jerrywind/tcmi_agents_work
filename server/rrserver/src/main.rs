//! rrserver CLI：云端中继服务器（`server`）与家庭端隧道客户端（`client`）。

use anyhow::Context;
use clap::{Parser, Subcommand};
use std::sync::Arc;
use std::time::Duration;
use toml::Value;

use rrserver::skill::{ConstState, JudgeEngine, SkillRule, SkillSet};
use rrserver::{client, llmsrv, server, state};

use server::{AppState, TunnelAuth};

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

/// `load_config` 的返回：`(隧道 name/token 列表, external_ws_base, 可选技能集)`
type LoadedConfig = (Vec<(String, String)>, String, Option<Arc<SkillSet>>);

fn load_config(path: Option<&str>) -> anyhow::Result<LoadedConfig> {
    let path = match path {
        Some(p) => p,
        None => return Ok((vec![], String::new(), None)),
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

    Ok((tokens, ws_base, skills))
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
            let (tokens, cfg_ws_base, skills) = load_config(config.as_deref())?;
            if tokens.is_empty() {
                anyhow::bail!("no tunnels configured; add [[tunnels]] to config");
            }
            let ws_base = if cfg_ws_base.is_empty() {
                external_ws_base
            } else {
                cfg_ws_base
            };
            let auth = TunnelAuth::from_list(&tokens);
            let state = AppState {
                registry: state::Registry::new(),
                auth,
                external_ws_base: ws_base,
                skills,
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
        let (tunnels, ws_base, skills) = load_config(Some(p)).unwrap();
        std::fs::remove_file(p).ok();
        assert_eq!(tunnels.len(), 2);
        assert_eq!(tunnels[0], ("home".to_string(), "s3cr3t".to_string()));
        assert_eq!(tunnels[1], ("other".to_string(), "othertok".to_string()));
        assert_eq!(ws_base, "wss://rr.example.com/rr");
        assert!(skills.is_none());
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
        let (tunnels, _ws, skills) = load_config(Some(p)).unwrap();
        std::fs::remove_file(p).ok();
        assert_eq!(tunnels.len(), 1);
        let set = skills.expect("skills should be enabled");
        assert!(set.contains("fire"));
        let rule = set.get("fire").unwrap();
        assert_eq!(rule.cooldown, Duration::from_secs(30));
        assert_eq!(rule.cost, 2);
    }

    #[test]
    fn load_config_missing_path_returns_empty() {
        let (tunnels, ws_base, skills) = load_config(None).unwrap();
        assert!(tunnels.is_empty());
        assert!(ws_base.is_empty());
        assert!(skills.is_none());
    }
}
