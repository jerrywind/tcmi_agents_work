//! `llm_server` 模型部署包装与 rrserver 注册。
//!
//! 本模块把「部署一个本地模型服务」与「把它注册到云端 rrserver 隧道」这两件事封装为一个可运行的包装器：
//!
//! 1. **模型部署包装（Deployer）**：支持两种后端形态
//!    - `static`：接入一个已经运行的本地模型服务（如已手动启动的 vLLM / Ollama），不启动新进程；
//!    - `command`：由本包装器 `spawn` 一个模型服务进程并持续监管，启动后轮询健康检查探针直到就绪。
//! 2. **rrserver 注册**：后端就绪后，组合出 `client::ClientConfig` 并运行 `client::run_client`，
//!    把本地模型服务通过反向隧道暴露到云端（外部访问 `https://<域名>/rr/t/<name>/v1` 即达本地模型）。
//! 3. **可选 info 端点**：包装器自身可暴露 `/healthz` 与 OpenAI 风格的 `/v1/models`，便于容器探活与模型发现。
//!
//! 单一职责：本模块负责「部署生命周期 + 注册编排」，具体的隧道收发逻辑全部委托给 `crate::client`。

use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, Context};
use axum::{extract::Json, routing::get, Router};
use serde::Deserialize;
use tokio::process::Command as TokioCommand;
use tracing::{error, info, warn};

use crate::client::{self, ClientConfig};

/// 一个模型的元信息（仅用于 info 端点的 /v1/models 展示）。
#[derive(Debug, Clone, Deserialize)]
pub struct ModelMeta {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub context_length: Option<u64>,
}

/// 后端形态：静态接入 或 由本包装器启动并监管。
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Backend {
    /// 接入一个已经运行的本地模型服务（不启动新进程）。
    Static { url: String },
    /// 由本包装器启动并监管一个模型服务进程。
    Command {
        program: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: Vec<EnvPair>,
        #[serde(default)]
        cwd: Option<String>,
        /// 健康检查探针 URL，返回 2xx 视为就绪。
        health_url: String,
        /// 隧道应转发的模型服务基址；缺省时从 `health_url` 推导（scheme://authority）。
        #[serde(default)]
        listen_url: Option<String>,
    },
}

/// 环境变量键值对（TOML 中表示 `[[backend.env]]`）。
#[derive(Debug, Clone, Deserialize)]
pub struct EnvPair {
    pub key: String,
    pub value: String,
}

/// 后端配置：形态 + 健康检查超时。
#[derive(Debug, Clone, Deserialize)]
pub struct BackendConfig {
    /// 后端形态。使用 `flatten` 让 `mode`/`url`/`program` 等字段直接平铺在 `[backend]` 表内，
    /// 从而使配置格式与示例 `config/llm_server.toml.example` 一致（无需嵌套子表）。
    #[serde(flatten)]
    pub backend: Backend,
    /// 健康检查超时（秒），默认 120。
    #[serde(default = "default_health_timeout")]
    pub health_timeout_secs: u64,
}

fn default_health_timeout() -> u64 {
    120
}

/// 注册到云端 rrserver 的隧道凭据（须与云端 `[[tunnels]]` 一致）。
#[derive(Debug, Clone, Deserialize)]
pub struct RrClientConfig {
    pub server_base: String,
    pub name: String,
    pub token: String,
}

/// 完整的部署配置（对应一份 TOML）。
#[derive(Debug, Clone, Deserialize)]
pub struct DeploymentConfig {
    pub backend: BackendConfig,
    pub rrclient: RrClientConfig,
    /// 模型元信息（可选，供 info 端点展示）。
    #[serde(default)]
    pub models: Vec<ModelMeta>,
    /// 包装器自身暴露 `/healthz` 与 `/v1/models` 的端口；缺省不启动。
    #[serde(default)]
    pub info_port: Option<u16>,
}

impl DeploymentConfig {
    /// 从 TOML 文件加载部署配置。
    pub fn from_toml(path: &str) -> anyhow::Result<Self> {
        let content =
            std::fs::read_to_string(path).with_context(|| format!("read config {}", path))?;
        let cfg: DeploymentConfig = toml::from_str(&content).context("parse llm_server config")?;
        Ok(cfg)
    }

    /// 用 CLI 覆盖项覆盖 rrclient 凭据（只覆盖提供的字段）。
    pub fn apply_overrides(
        &mut self,
        server: Option<String>,
        name: Option<String>,
        token: Option<String>,
    ) {
        if let Some(s) = server {
            self.rrclient.server_base = s;
        }
        if let Some(n) = name {
            self.rrclient.name = n;
        }
        if let Some(t) = token {
            self.rrclient.token = t;
        }
    }
}

/// 部署器：负责把后端拉起并就绪。
pub struct Deployer {
    cfg: DeploymentConfig,
}

/// 已部署后端：持有被监管的子进程，析构时杀死它。
pub struct Deployed {
    /// 隧道应转发的模型服务基址。
    pub local_url: String,
    child: Option<tokio::process::Child>,
}

impl Deployed {
    /// 显式杀死被监管的后端进程并等待其退出；返回退出状态（若有）。
    pub async fn kill(&mut self) -> anyhow::Result<Option<std::process::ExitStatus>> {
        if let Some(mut c) = self.child.take() {
            c.kill().await.context("kill backend child")?;
            let status = c.wait().await.ok();
            return Ok(status);
        }
        Ok(None)
    }
}

impl Drop for Deployed {
    fn drop(&mut self) {
        // 同步尽力杀死子进程；忽略错误（进程可能已自行退出）。
        if let Some(mut c) = self.child.take() {
            let _ = c.start_kill();
        }
    }
}

impl Deployer {
    pub fn new(cfg: DeploymentConfig) -> Self {
        Self { cfg }
    }

    /// 部署后端：静态模式直接返回基址；命令模式启动进程并等待健康检查通过。
    pub async fn deploy(&self) -> anyhow::Result<Deployed> {
        match &self.cfg.backend.backend {
            Backend::Static { url } => Ok(Deployed {
                local_url: url.clone(),
                child: None,
            }),
            Backend::Command {
                program,
                args,
                env,
                cwd,
                health_url,
                listen_url,
            } => {
                let mut cmd = TokioCommand::new(program);
                cmd.args(args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit());
                for e in env {
                    cmd.env(&e.key, &e.value);
                }
                if let Some(c) = cwd {
                    cmd.current_dir(c);
                }
                let child = cmd
                    .spawn()
                    .with_context(|| format!("spawn backend `{}`", program))?;
                info!(
                    "backend process spawned (pid {:?})",
                    child.id()
                );
                let local_url = match listen_url {
                    Some(u) => u.clone(),
                    None => derive_base(health_url)?,
                };
                self.wait_until_ready(
                    health_url,
                    Duration::from_secs(self.cfg.backend.health_timeout_secs),
                )
                .await?;
                info!("backend ready at {}", local_url);
                Ok(Deployed {
                    local_url,
                    child: Some(child),
                })
            }
        }
    }

    /// 轮询健康检查探针直到返回 2xx 或超时。
    async fn wait_until_ready(&self, health_url: &str, timeout: Duration) -> anyhow::Result<()> {
        let client = reqwest::Client::new();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match client.get(health_url).send().await {
                Ok(r) if r.status().is_success() => return Ok(()),
                _ => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(anyhow!(
                            "backend not ready within {:?} (probe {})",
                            timeout,
                            health_url
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }

    /// 组合出注册到 rrserver 用的 `ClientConfig`。
    pub fn build_client_config(&self, local_url: &str) -> ClientConfig {
        ClientConfig {
            server_base: self.cfg.rrclient.server_base.clone(),
            name: self.cfg.rrclient.name.clone(),
            token: self.cfg.rrclient.token.clone(),
            local_url: local_url.to_string(),
        }
    }
}

/// 从带路径的 URL 推导出 `scheme://authority` 基址。
fn derive_base(url: &str) -> anyhow::Result<String> {
    let (scheme, after) = url
        .split_once("://")
        .ok_or_else(|| anyhow!("invalid health_url (no scheme): {}", url))?;
    let authority = after.split('/').next().unwrap_or(after);
    Ok(format!("{}://{}", scheme, authority))
}

/// 启动包装器自身的 info 服务（/healthz + /v1/models）。
/// `port` 为 `Some(p)` 时绑定 `0.0.0.0:p`，否则绑定 `127.0.0.1:0`（仅本机）。
/// 返回实际绑定的地址，便于测试或日志。
pub async fn start_info_server(
    models: &[ModelMeta],
    port: Option<u16>,
) -> anyhow::Result<std::net::SocketAddr> {
    let data: Vec<serde_json::Value> = models
        .iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "object": "model",
                "display_name": m.display_name,
                "context_length": m.context_length,
            })
        })
        .collect();
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/v1/models",
            get(move || async move {
                Json(serde_json::json!({ "object": "list", "data": data }))
            }),
        );
    let bind: (String, u16) = match port {
        Some(p) => ("0.0.0.0".to_string(), p),
        None => ("127.0.0.1".to_string(), 0),
    };
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("bind info server {:?}", bind))?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(addr)
}

/// 运行完整的「部署 + 注册」流程：
/// 1. 部署后端（启动进程 / 接入已有服务并等待就绪）；
/// 2. 可选启动 info 服务；
/// 3. 注册到 rrserver（在后台运行隧道客户端）；
/// 4. 等待 Ctrl-C，收到后杀死后端进程并退出。
pub async fn run(cfg: DeploymentConfig) -> anyhow::Result<()> {
    let deployer = Deployer::new(cfg.clone());
    let mut deployed = deployer.deploy().await?;
    let local = deployed.local_url.clone();

    if let Some(port) = cfg.info_port {
        let models = cfg.models.clone();
        let addr = start_info_server(&models, Some(port)).await?;
        info!("llm_server info endpoint on http://{}", addr);
    }

    let client_cfg = deployer.build_client_config(&local);
    info!(
        "registering with rrserver as tunnel `{}` -> {}",
        client_cfg.name, local
    );
    tokio::spawn(async move {
        if let Err(e) = client::run_client(client_cfg).await {
            error!("tunnel client exited: {:#}", e);
        }
    });

    // 前台等待退出信号；收到后析构 deployed（杀死后端进程）。
    let _ = tokio::signal::ctrl_c().await;
    info!("shutting down; stopping backend...");
    deployed.kill().await?;
    warn!("llm_server stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Router};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

    /// 把 TOML 内容写到临时文件，返回路径（测试结束后由调用方清理）。
    fn write_temp_toml(content: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        let name = format!(
            "rrsrv_cfg_{}_{}.toml",
            std::process::id(),
            TMP_SEQ.fetch_add(1, Ordering::SeqCst),
        );
        path.push(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    /// 构造一个 Static 后端的配置，便于覆盖逻辑测试。
    fn static_cfg(url: &str) -> DeploymentConfig {
        DeploymentConfig {
            backend: BackendConfig {
                backend: Backend::Static {
                    url: url.to_string(),
                },
                health_timeout_secs: 5,
            },
            rrclient: RrClientConfig {
                server_base: "https://rr.example.com/rr".into(),
                name: "home".into(),
                token: "tok".into(),
            },
            models: vec![],
            info_port: None,
        }
    }

    #[tokio::test]
    async fn static_backend_resolves_local_url_without_spawn() {
        let d = Deployer::new(static_cfg("http://127.0.0.1:8080"));
        let deployed = d.deploy().await.unwrap();
        assert_eq!(deployed.local_url, "http://127.0.0.1:8080");
        assert!(deployed.child.is_none());
    }

    #[tokio::test]
    async fn wait_until_ready_succeeds_for_healthy_server() {
        let app = Router::new().route("/healthz", get(|| async { "ok" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let d = Deployer::new(static_cfg("x"));
        d.wait_until_ready(
            &format!("http://{}/healthz", addr),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn wait_until_ready_times_out_for_dead_endpoint() {
        let d = Deployer::new(static_cfg("x"));
        let res = d
            .wait_until_ready("http://127.0.0.1:1/healthz", Duration::from_millis(800))
            .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn deploy_command_supervises_child_and_kills_on_kill() {
        // 启动一个跨平台的长驻进程，健康检查指向一个就绪的本地服务。
        let app = Router::new().route("/healthz", get(|| async { "ok" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let (prog, args) = if cfg!(windows) {
            (
                "ping".to_string(),
                vec!["-n".to_string(), "60".to_string(), "127.0.0.1".to_string()],
            )
        } else {
            ("sleep".to_string(), vec!["60".to_string()])
        };
        let cfg = DeploymentConfig {
            backend: BackendConfig {
                backend: Backend::Command {
                    program: prog,
                    args,
                    env: vec![],
                    cwd: None,
                    health_url: format!("http://{}/healthz", addr),
                    listen_url: Some("http://127.0.0.1:9".to_string()),
                },
                health_timeout_secs: 10,
            },
            rrclient: RrClientConfig {
                server_base: "x".into(),
                name: "home".into(),
                token: "t".into(),
            },
            models: vec![],
            info_port: None,
        };
        let d = Deployer::new(cfg);
        let mut deployed = d.deploy().await.unwrap();
        assert_eq!(deployed.local_url, "http://127.0.0.1:9");
        assert!(deployed.child.is_some());
        // 显式 kill 应成功，且子进程已退出
        let status = deployed.kill().await.unwrap();
        assert!(status.is_some(), "child should have exited after kill");
        assert!(deployed.child.is_none());
    }

    #[test]
    fn build_client_config_composes_local_url() {
        let d = Deployer::new(static_cfg("http://127.0.0.1:8080"));
        let cc = d.build_client_config("http://127.0.0.1:8080");
        assert_eq!(cc.server_base, "https://rr.example.com/rr");
        assert_eq!(cc.name, "home");
        assert_eq!(cc.token, "tok");
        assert_eq!(cc.local_url, "http://127.0.0.1:8080");
    }

    #[test]
    fn derive_base_strips_path() {
        assert_eq!(
            derive_base("http://127.0.0.1:8080/health").unwrap(),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            derive_base("https://host:9000/v1/models").unwrap(),
            "https://host:9000"
        );
    }

    #[tokio::test]
    async fn info_server_exposes_health_and_models() {
        let models = vec![ModelMeta {
            id: "llama3".into(),
            display_name: Some("Llama3".into()),
            context_length: Some(8192),
        }];
        let addr = start_info_server(&models, None).await.unwrap();
        let client = reqwest::Client::new();
        let h = client
            .get(format!("http://{}/healthz", addr))
            .send()
            .await
            .unwrap();
        assert_eq!(h.status(), 200);
        let m = client
            .get(format!("http://{}/v1/models", addr))
            .send()
            .await
            .unwrap();
        assert_eq!(m.status(), 200);
        let body: serde_json::Value = m.json().await.unwrap();
        assert_eq!(body["data"][0]["id"], "llama3");
    }

    #[tokio::test]
    async fn from_toml_parses_static_backend() {
        let toml = r#"
[backend]
mode = "static"
url = "http://127.0.0.1:8080"
health_timeout_secs = 30

[rrclient]
server_base = "https://rr.example.com/rr"
name = "home"
token = "tok"

[[models]]
id = "llama3"
display_name = "Llama3"
context_length = 8192
"#;
        let path = write_temp_toml(toml);
        let cfg = DeploymentConfig::from_toml(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(cfg.backend.health_timeout_secs, 30);
        assert_eq!(cfg.models.len(), 1);
        assert_eq!(cfg.models[0].id, "llama3");
        match &cfg.backend.backend {
            Backend::Static { url } => assert_eq!(url, "http://127.0.0.1:8080"),
            other => panic!("expected static backend, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn from_toml_parses_command_backend() {
        let toml = r#"
[backend]
mode = "command"
program = "python"
args = ["-m", "http.server", "8080"]
health_url = "http://127.0.0.1:8080/healthz"
listen_url = "http://127.0.0.1:8080"
health_timeout_secs = 15

[rrclient]
server_base = "https://rr.example.com/rr"
name = "home"
token = "tok"
"#;
        let path = write_temp_toml(toml);
        let cfg = DeploymentConfig::from_toml(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(cfg.backend.health_timeout_secs, 15);
        match &cfg.backend.backend {
            Backend::Command {
                program,
                args,
                health_url,
                listen_url,
                ..
            } => {
                assert_eq!(program, "python");
                assert_eq!(
                    args,
                    &[
                        "-m".to_string(),
                        "http.server".to_string(),
                        "8080".to_string()
                    ]
                );
                assert_eq!(health_url, "http://127.0.0.1:8080/healthz");
                assert_eq!(listen_url.as_deref(), Some("http://127.0.0.1:8080"));
            }
            other => panic!("expected command backend, got {:?}", other),
        }
    }

    #[test]
    fn apply_overrides_only_sets_provided_fields() {
        let mut cfg = static_cfg("http://127.0.0.1:8080");
        cfg.apply_overrides(Some("https://new.example.com/rr".into()), None, None);
        assert_eq!(cfg.rrclient.server_base, "https://new.example.com/rr");
        assert_eq!(cfg.rrclient.name, "home"); // 未被覆盖
        assert_eq!(cfg.rrclient.token, "tok"); // 未被覆盖
    }

    #[test]
    fn apply_overrides_all_none_keeps_original() {
        let mut cfg = static_cfg("http://127.0.0.1:8080");
        cfg.apply_overrides(None, None, None);
        assert_eq!(cfg.rrclient.server_base, "https://rr.example.com/rr");
        assert_eq!(cfg.rrclient.name, "home");
        assert_eq!(cfg.rrclient.token, "tok");
    }
}
