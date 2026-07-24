use std::{
    collections::BTreeMap,
    env,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

use anyhow::{Context, bail};
use clap::{Parser, ValueEnum};
use reqwest::Client;
use serde_json::Value;

#[derive(Debug, Parser)]
#[command(
    name = "sessionweft-agent-server",
    version,
    about = "Launch wrapped agent terminals that share one durable SessionWeft Session"
)]
struct Cli {
    #[arg(
        long,
        env = "SESSIONWEFT_ENDPOINT",
        default_value = "http://127.0.0.1:7447"
    )]
    endpoint: String,

    #[arg(long, env = "SESSIONWEFT_API_TOKEN", hide_env_values = true)]
    token: Option<String>,

    #[arg(long, default_value = ".")]
    cwd: PathBuf,

    #[arg(long, env = "SESSIONWEFT_WRAPPER_BIN_DIR")]
    bin_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum AgentKind {
    Codex,
    Claude,
    Gemini,
    FccClaude,
    FccCodex,
    Antigravity,
}

impl AgentKind {
    const ALL: [Self; 6] = [
        Self::Codex,
        Self::Claude,
        Self::Gemini,
        Self::FccClaude,
        Self::FccCodex,
        Self::Antigravity,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::FccClaude => "fcc-claude",
            Self::FccCodex => "fcc-codex",
            Self::Antigravity => "antigravity",
        }
    }

    const fn launcher(self) -> &'static str {
        match self {
            Self::Codex => "sw-codex",
            Self::Claude => "sw-claude",
            Self::Gemini => "sw-gemini",
            Self::FccClaude => "sw-fcc-claude",
            Self::FccCodex => "sw-fcc-codex",
            Self::Antigravity => "sw-antigravity",
        }
    }

    const fn program(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::FccClaude => "fcc-claude",
            Self::FccCodex => "fcc-codex",
            Self::Antigravity => "antigravity-ide",
        }
    }
}

#[derive(Clone)]
struct RuntimeClient {
    http: Client,
    endpoint: String,
    token: Option<String>,
}

impl RuntimeClient {
    fn new(endpoint: String, token: Option<String>) -> Self {
        Self {
            http: Client::new(),
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            token,
        }
    }

    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        let mut request = self.http.get(format!("{}{}", self.endpoint, path));
        if let Some(token) = self.token.as_deref() {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.context("reach SessionWeft Runtime")?;
        let status = response.status();
        let value = response
            .json::<Value>()
            .await
            .context("decode Runtime response")?;
        if !status.is_success() {
            bail!("SessionWeft Runtime returned HTTP {status}: {value}");
        }
        Ok(value)
    }
}

#[tokio::main]
pub(super) async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cwd = std::fs::canonicalize(&cli.cwd)
        .with_context(|| format!("resolve wrapper working directory {}", cli.cwd.display()))?;
    let bin_dir = cli
        .bin_dir
        .or_else(|| {
            env::current_exe()
                .ok()
                .and_then(|path| path.parent().map(Path::to_owned))
        })
        .context("resolve wrapper binary directory")?;
    let runtime = RuntimeClient::new(cli.endpoint, cli.token);

    println!("SessionWeft agent wrapper server");
    println!("Runtime: {}", runtime.endpoint);
    println!("Workspace: {}", cwd.display());
    println!("Launchers:");
    for agent in AgentKind::ALL {
        println!(
            "  {:<18} -> {:<16} ({})",
            agent.launcher(),
            agent.program(),
            agent.label()
        );
    }
    println!();
    println!(
        "This process keeps wrapper configuration available. Launchers are standalone binaries that connect to the same Runtime."
    );

    let health = runtime.get("/health/ready").await?;
    println!(
        "Runtime health: {}",
        health
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("ready")
    );

    install_launcher_hints(&bin_dir)?;

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    loop {
        write!(stdout, "server> ")?;
        stdout.flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }
        match line.trim() {
            "" => {}
            "help" => println!("commands: help, sessions, launchers, quit"),
            "sessions" => print_sessions(&runtime.get("/v1/sessions?limit=100").await?)?,
            "launchers" => {
                for agent in AgentKind::ALL {
                    println!("{} ({})", agent.launcher(), agent.label());
                }
            }
            "quit" | "exit" => break,
            other => eprintln!("unknown server command: {other}"),
        }
    }
    Ok(())
}

fn install_launcher_hints(bin_dir: &Path) -> anyhow::Result<()> {
    let wrapper = bin_dir.join("sessionweft-agent-wrapper");
    println!("Wrapper binary: {}", wrapper.display());
    println!("Create symlinks or aliases pointing these names to the wrapper binary:");
    for agent in AgentKind::ALL {
        println!(
            "  ln -sf {} {}",
            wrapper.display(),
            bin_dir.join(agent.launcher()).display()
        );
    }
    Ok(())
}

fn print_sessions(value: &Value) -> anyhow::Result<()> {
    let sessions = value
        .as_array()
        .context("Session list response is not an array")?;
    println!("SESSIONS ({})", sessions.len());
    for session in sessions {
        let id = session.get("id").and_then(Value::as_str).unwrap_or("-");
        let title = session
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Untitled Session");
        let version = session.get("version").and_then(Value::as_u64).unwrap_or(0);
        println!("  {id}  v{version}  {title}");
    }
    Ok(())
}

#[allow(dead_code)]
fn _native_terminal_contract() -> BTreeMap<&'static str, &'static str> {
    AgentKind::ALL
        .into_iter()
        .map(|agent| (agent.launcher(), agent.program()))
        .collect()
}

#[allow(dead_code)]
fn _spawn_native_program(agent: AgentKind, cwd: &Path) -> anyhow::Result<()> {
    let status = Command::new(agent.program())
        .current_dir(cwd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("launch wrapped agent {}", agent.program()))?;
    if !status.success() {
        bail!("wrapped agent {} exited with {status}", agent.program());
    }
    Ok(())
}

#[allow(dead_code)]
fn _shared_selection() -> Arc<Mutex<Option<String>>> {
    Arc::new(Mutex::new(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_names_are_unique() {
        let mut names = AgentKind::ALL
            .into_iter()
            .map(AgentKind::launcher)
            .collect::<Vec<_>>();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), AgentKind::ALL.len());
    }

    #[test]
    fn every_launcher_maps_to_real_program() {
        for agent in AgentKind::ALL {
            assert!(!agent.launcher().is_empty());
            assert!(!agent.program().is_empty());
            assert!(!agent.label().is_empty());
        }
    }
}
