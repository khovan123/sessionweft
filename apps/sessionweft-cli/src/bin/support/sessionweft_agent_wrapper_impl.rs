use std::{
    env,
    ffi::OsString,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, bail};
use clap::Parser;
use reqwest::Client;
use serde_json::{Value, json};

#[derive(Debug, Parser)]
#[command(
    name = "sessionweft-agent-wrapper",
    version,
    about = "Select or create a shared Session, then open the native agent CLI"
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

    #[arg(long)]
    new: Option<String>,

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
        decode_response(request.send().await.context("reach SessionWeft Runtime")?).await
    }

    async fn post(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        let mut request = self
            .http
            .post(format!("{}{}", self.endpoint, path))
            .json(&body);
        if let Some(token) = self.token.as_deref() {
            request = request.bearer_auth(token);
        }
        decode_response(request.send().await.context("reach SessionWeft Runtime")?).await
    }

    async fn create_session(&self, title: &str) -> anyhow::Result<Value> {
        self.post("/v1/sessions", json!({"title": title})).await
    }
}

async fn decode_response(response: reqwest::Response) -> anyhow::Result<Value> {
    let status = response.status();
    let value = response.json::<Value>().await.context("decode Runtime response")?;
    if !status.is_success() {
        bail!("SessionWeft Runtime returned HTTP {status}: {value}");
    }
    Ok(value)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cwd = std::fs::canonicalize(&cli.cwd)
        .with_context(|| format!("resolve wrapper working directory {}", cli.cwd.display()))?;
    let agent = resolve_agent(cli.agent.as_deref())?;
    let runtime = RuntimeClient::new(cli.endpoint, cli.token);

    let session = if let Some(session_id) = cli.session.as_deref() {
        runtime.get(&format!("/v1/sessions/{session_id}")).await?
    } else if let Some(title) = cli.new.as_deref() {
        runtime.create_session(title).await?
    } else {
        select_session(&runtime).await?
    };

    print_session(&session, agent);
    materialize_wrapper_context(&cwd, &session, agent)?;
    launch_native(agent, &cwd, &cli.passthrough)
}

async fn select_session(runtime: &RuntimeClient) -> anyhow::Result<Value> {
    println!("SessionWeft session setup");
    println!("  [n] Create new Session");
    println!("  [r] Resume existing Session");
    print!("Choose [n/r]: ");
    io::stdout().flush()?;

    let choice = read_line()?.to_ascii_lowercase();
    match choice.as_str() {
        "n" | "new" | "1" => {
            print!("Session title [Shared coding session]: ");
            io::stdout().flush()?;
            let title = read_line()?;
            let title = if title.is_empty() {
                "Shared coding session"
            } else {
                title.as_str()
            };
            runtime.create_session(title).await
        }
        "r" | "resume" | "2" => {
            let sessions = runtime.get("/v1/sessions?limit=100").await?;
            print_sessions(&sessions)?;
            print!("Session ID: ");
            io::stdout().flush()?;
            let session_id = read_line()?;
            if session_id.is_empty() {
                bail!("Session ID is required");
            }
            runtime.get(&format!("/v1/sessions/{session_id}")).await
        }
        other => bail!("unsupported choice '{other}'; use n or r"),
    }
}

fn read_line() -> anyhow::Result<String> {
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_owned())
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

fn launch_native(agent: AgentKind, cwd: &Path, args: &[OsString]) -> anyhow::Result<()> {
    let status = Command::new(agent.program())
        .args(args)
        .current_dir(cwd)
        .env("SESSIONWEFT_WRAPPED_AGENT", agent.label())
        .env("SESSIONWEFT_CONTEXT_FILE", cwd.join(".sessionweft/active-context.md"))
        .env("SESSIONWEFT_SESSION_FILE", cwd.join(".sessionweft/active-session"))
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
    println!("SESSION");
    println!("  id:      {}", session.get("id").and_then(Value::as_str).unwrap_or("-"));
    println!(
        "  title:   {}",
        session
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Untitled Session")
    );
    println!(
        "  version: {}",
        session.get("version").and_then(Value::as_u64).unwrap_or(0)
    );
    println!("  agent:   {}", agent.label());
    println!("Opening native {} CLI...\n", agent.label());
}

fn required_string(value: &Value, field: &str) -> anyhow::Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("Runtime response is missing '{field}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_aliases_map_to_native_agents() {
        assert_eq!(AgentKind::parse("sw-codex").unwrap(), AgentKind::Codex);
        assert_eq!(
            AgentKind::parse("sw-fcc-claude").unwrap(),
            AgentKind::FccClaude
        );
    }
}
