use std::{
    env,
    ffi::OsString,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, bail};
use clap::Parser;
use reqwest::Client;
use serde_json::Value;

#[derive(Debug, Parser)]
#[command(
    name = "sessionweft-agent-wrapper",
    version,
    about = "Wrap a real agent terminal with shared SessionWeft /resume commands"
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

    #[arg(long)]
    session: Option<String>,

    #[arg(long, default_value = ".")]
    cwd: PathBuf,

    #[arg(long)]
    agent: Option<String>,

    #[arg(last = true)]
    passthrough: Vec<OsString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentKind {
    Codex,
    Claude,
    Gemini,
    FccClaude,
    FccCodex,
    Antigravity,
}

impl AgentKind {
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

    fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "codex" | "sw-codex" => Ok(Self::Codex),
            "claude" | "claude-code" | "sw-claude" => Ok(Self::Claude),
            "gemini" | "gemini-cli" | "sw-gemini" => Ok(Self::Gemini),
            "fcc-claude" | "sw-fcc-claude" => Ok(Self::FccClaude),
            "fcc-codex" | "sw-fcc-codex" => Ok(Self::FccCodex),
            "antigravity" | "antigravity-ide" | "sw-antigravity" => Ok(Self::Antigravity),
            other => bail!("unsupported wrapped agent '{other}'"),
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
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cwd = std::fs::canonicalize(&cli.cwd)
        .with_context(|| format!("resolve wrapper working directory {}", cli.cwd.display()))?;
    let agent = resolve_agent(cli.agent.as_deref())?;
    let runtime = RuntimeClient::new(cli.endpoint, cli.token);

    let mut session_id = cli.session;
    if session_id.is_none() {
        print_sessions(&runtime.get("/v1/sessions?limit=100").await?)?;
        session_id = prompt_session_id()?;
    }
    let selected = session_id.context("a Session ID is required")?;
    let session = runtime.get(&format!("/v1/sessions/{selected}")).await?;
    print_session(&session, agent);
    materialize_wrapper_context(&cwd, &session, agent)?;

    if matches!(agent, AgentKind::Antigravity) {
        return launch_native(agent, &cwd, &cli.passthrough);
    }

    println!("SessionWeft wrapper commands are available before the native agent starts:");
    println!("  /resume                 list Sessions");
    println!("  /resume <SESSION_ID>    select Session");
    println!("  /session                show current Session");
    println!("  /start                  open the native {} terminal", agent.label());
    println!();

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut current = selected;
    loop {
        write!(stdout, "[{} | {} wrapper] > ", short_id(&current), agent.label())?;
        stdout.flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "/resume" {
            print_sessions(&runtime.get("/v1/sessions?limit=100").await?)?;
        } else if let Some(id) = line.strip_prefix("/resume ") {
            let session = runtime
                .get(&format!("/v1/sessions/{}", id.trim()))
                .await?;
            current = required_string(&session, "id")?;
            print_session(&session, agent);
            materialize_wrapper_context(&cwd, &session, agent)?;
        } else if line == "/session" {
            let session = runtime.get(&format!("/v1/sessions/{current}")).await?;
            print_session(&session, agent);
        } else if line == "/start" {
            return launch_native(agent, &cwd, &cli.passthrough);
        } else if matches!(line, "/quit" | "/exit") {
            break;
        } else {
            eprintln!("unknown wrapper command: {line}; use /resume, /session or /start");
        }
    }
    Ok(())
}

fn resolve_agent(explicit: Option<&str>) -> anyhow::Result<AgentKind> {
    if let Some(value) = explicit {
        return AgentKind::parse(value);
    }
    let executable = env::args_os()
        .next()
        .and_then(|path| PathBuf::from(path).file_name().map(OsString::from))
        .and_then(|name| name.into_string().ok())
        .context("resolve wrapper executable name")?;
    AgentKind::parse(&executable)
}

fn prompt_session_id() -> anyhow::Result<Option<String>> {
    print!("Session ID: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();
    Ok((!input.is_empty()).then(|| input.to_owned()))
}

fn launch_native(agent: AgentKind, cwd: &Path, args: &[OsString]) -> anyhow::Result<()> {
    let status = Command::new(agent.program())
        .args(args)
        .current_dir(cwd)
        .env("SESSIONWEFT_WRAPPED_AGENT", agent.label())
        .env("SESSIONWEFT_CONTEXT_FILE", cwd.join(".sessionweft/active-context.md"))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("launch native wrapped agent {}", agent.program()))?;
    if !status.success() {
        bail!("native wrapped agent {} exited with {status}", agent.program());
    }
    Ok(())
}

fn materialize_wrapper_context(cwd: &Path, session: &Value, agent: AgentKind) -> anyhow::Result<()> {
    let directory = cwd.join(".sessionweft");
    std::fs::create_dir_all(&directory)?;
    let id = required_string(session, "id")?;
    let title = session
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Untitled Session");
    let version = session.get("version").and_then(Value::as_u64).unwrap_or(0);
    let mut content = format!(
        "# SessionWeft active context\n\n- Session ID: `{id}`\n- Title: {title}\n- Version: {version}\n- Agent: {}\n\n## Shared history\n",
        agent.label()
    );
    if let Some(messages) = session.get("messages").and_then(Value::as_array) {
        for message in messages.iter().rev().take(200).rev() {
            let role = message.get("role").and_then(Value::as_str).unwrap_or("unknown");
            let body = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            content.push_str(&format!("\n### {role}\n\n{body}\n"));
        }
    }
    std::fs::write(directory.join("active-context.md"), content)?;
    std::fs::write(directory.join("active-session"), id)?;
    Ok(())
}

fn print_sessions(value: &Value) -> anyhow::Result<()> {
    let sessions = value.as_array().context("Session list is not an array")?;
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

fn print_session(session: &Value, agent: AgentKind) {
    let id = session.get("id").and_then(Value::as_str).unwrap_or("-");
    let title = session
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Untitled Session");
    let version = session.get("version").and_then(Value::as_u64).unwrap_or(0);
    println!("SESSION");
    println!("  id:      {id}");
    println!("  title:   {title}");
    println!("  version: {version}");
    println!("  agent:   {}", agent.label());
}

fn required_string(value: &Value, field: &str) -> anyhow::Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("Runtime response is missing '{field}'"))
}

fn short_id(value: &str) -> &str {
    value.get(..8).unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_aliases_map_to_native_agents() {
        assert_eq!(AgentKind::parse("sw-codex").unwrap(), AgentKind::Codex);
        assert_eq!(AgentKind::parse("sw-claude").unwrap(), AgentKind::Claude);
        assert_eq!(
            AgentKind::parse("sw-antigravity").unwrap(),
            AgentKind::Antigravity
        );
        assert_eq!(
            AgentKind::parse("sw-fcc-claude").unwrap(),
            AgentKind::FccClaude
        );
    }
}
