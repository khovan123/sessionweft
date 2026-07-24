use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Context, bail};
use clap::{Parser, ValueEnum};
use reqwest::{Client, Method};
use serde_json::{Value, json};
use tokio::{
    io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

#[derive(Debug, Parser)]
#[command(
    name = "sessionweft-agent-shell",
    version,
    about = "Use /resume and one shared durable Session across all SessionWeft agent adapters"
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

    #[arg(value_enum, default_value_t = AgentKind::Codex)]
    agent: AgentKind,

    #[arg(long)]
    session: Option<String>,

    #[arg(long, default_value = "Shared agent shell")]
    title: String,

    #[arg(long, default_value = ".")]
    cwd: PathBuf,

    #[arg(
        long,
        env = "SESSIONWEFT_AGENT_BIN",
        default_value = "sessionweft-agent"
    )]
    agent_bin: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum AgentKind {
    Codex,
    Claude,
    Gemini,
    Antigravity,
    FccClaude,
    FccCodex,
}

impl AgentKind {
    const ALL: [Self; 6] = [
        Self::Codex,
        Self::Claude,
        Self::Gemini,
        Self::Antigravity,
        Self::FccClaude,
        Self::FccCodex,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::Antigravity => "antigravity",
            Self::FccClaude => "fcc-claude",
            Self::FccCodex => "fcc-codex",
        }
    }

    fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "codex" => Ok(Self::Codex),
            "claude" | "claude-code" => Ok(Self::Claude),
            "gemini" | "gemini-cli" => Ok(Self::Gemini),
            "antigravity" | "antigravity-ide" | "anti" => Ok(Self::Antigravity),
            "fcc-claude" => Ok(Self::FccClaude),
            "fcc-codex" => Ok(Self::FccCodex),
            other => bail!(
                "unsupported agent '{other}'; expected {}",
                Self::ALL
                    .into_iter()
                    .map(Self::label)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

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

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> anyhow::Result<Value> {
        let mut request = self
            .http
            .request(method, format!("{}{}", self.endpoint, path));
        if let Some(token) = self.token.as_deref() {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("failed to reach SessionWeft Runtime at {}", self.endpoint))?;
        let status = response.status();
        let bytes = response.bytes().await.context("read Runtime response")?;
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        if !status.is_success() {
            bail!("SessionWeft Runtime returned HTTP {status}: {value}");
        }
        Ok(value)
    }

    async fn create_session(&self, title: &str) -> anyhow::Result<Value> {
        self.request(Method::POST, "/v1/sessions", Some(json!({"title": title})))
            .await
    }

    async fn get_session(&self, session_id: &str) -> anyhow::Result<Value> {
        self.request(Method::GET, &format!("/v1/sessions/{session_id}"), None)
            .await
    }

    async fn list_sessions(&self, limit: u32) -> anyhow::Result<Value> {
        self.request(Method::GET, &format!("/v1/sessions?limit={limit}"), None)
            .await
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SlashCommand {
    Resume(Option<String>),
    Agent(Option<String>),
    New(Option<String>),
    Session,
    History,
    Context,
    Help,
    Quit,
    Unknown(String),
}

fn parse_slash_command(line: &str) -> Option<SlashCommand> {
    let trimmed = line.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    let (name, argument) = trimmed
        .split_once(char::is_whitespace)
        .map_or((trimmed, None), |(name, argument)| {
            let argument = argument.trim();
            (name, (!argument.is_empty()).then(|| argument.to_owned()))
        });
    Some(match name {
        "/resume" | "/sessions" => SlashCommand::Resume(argument),
        "/agent" => SlashCommand::Agent(argument),
        "/new" => SlashCommand::New(argument),
        "/session" => SlashCommand::Session,
        "/history" => SlashCommand::History,
        "/context" => SlashCommand::Context,
        "/help" => SlashCommand::Help,
        "/quit" | "/exit" => SlashCommand::Quit,
        other => SlashCommand::Unknown(other.to_owned()),
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cwd = tokio::fs::canonicalize(&cli.cwd)
        .await
        .with_context(|| format!("resolve working directory {}", cli.cwd.display()))?;
    let runtime = RuntimeClient::new(cli.endpoint.clone(), cli.token.clone());
    let agent_bin = resolve_agent_binary(&cli.agent_bin)?;

    let initial = match cli.session.as_deref() {
        Some(session_id) => runtime.get_session(session_id).await?,
        None => runtime.create_session(&cli.title).await?,
    };
    let mut session_id = required_string(&initial, "id")?;
    let mut agent = cli.agent;

    print_banner(&initial, agent, &cwd);
    print_help();

    let stdin = BufReader::new(io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = io::stdout();

    loop {
        stdout
            .write_all(format!("[{} | {}] > ", short_id(&session_id), agent.label()).as_bytes())
            .await?;
        stdout.flush().await?;

        let Some(line) = lines.next_line().await? else {
            break;
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        match parse_slash_command(line) {
            Some(SlashCommand::Resume(None)) => {
                print_sessions(&runtime.list_sessions(100).await?)?;
            }
            Some(SlashCommand::Resume(Some(selected))) => {
                let session = runtime.get_session(&selected).await?;
                session_id = required_string(&session, "id")?;
                print_selected_session(&session, agent);
            }
            Some(SlashCommand::Agent(None)) => print_agents(agent),
            Some(SlashCommand::Agent(Some(selected))) => {
                agent = AgentKind::parse(&selected)?;
                let session = runtime.get_session(&session_id).await?;
                print_selected_session(&session, agent);
            }
            Some(SlashCommand::New(title)) => {
                let title = title.as_deref().unwrap_or("Shared agent shell");
                let session = runtime.create_session(title).await?;
                session_id = required_string(&session, "id")?;
                print_selected_session(&session, agent);
            }
            Some(SlashCommand::Session) => {
                let session = runtime.get_session(&session_id).await?;
                print_selected_session(&session, agent);
            }
            Some(SlashCommand::History) => {
                let session = runtime.get_session(&session_id).await?;
                print_history(&session)?;
            }
            Some(SlashCommand::Context) => {
                let session = runtime.get_session(&session_id).await?;
                print_context(&session, agent)?;
            }
            Some(SlashCommand::Help) => print_help(),
            Some(SlashCommand::Quit) => break,
            Some(SlashCommand::Unknown(command)) => {
                eprintln!("unknown slash command: {command}; use /help");
            }
            None => {
                run_agent(
                    &agent_bin,
                    &cli.endpoint,
                    cli.token.as_deref(),
                    agent,
                    line,
                    &session_id,
                    &cwd,
                )
                .await?;
            }
        }
    }

    Ok(())
}

fn resolve_agent_binary(configured: &Path) -> anyhow::Result<PathBuf> {
    if configured.components().count() > 1 || configured.is_absolute() {
        return Ok(configured.to_owned());
    }
    let sibling = env::current_exe()
        .context("resolve current SessionWeft shell executable")?
        .parent()
        .map(|parent| parent.join(configured));
    Ok(sibling.filter(|path| path.is_file()).unwrap_or_else(|| configured.to_owned()))
}

async fn run_agent(
    agent_bin: &Path,
    endpoint: &str,
    token: Option<&str>,
    agent: AgentKind,
    prompt: &str,
    session_id: &str,
    cwd: &Path,
) -> anyhow::Result<()> {
    let mut command = Command::new(agent_bin);
    command
        .arg("--endpoint")
        .arg(endpoint)
        .arg("resume")
        .arg(agent.label())
        .arg(prompt)
        .arg("--session")
        .arg(session_id)
        .arg("--cwd")
        .arg(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(token) = token {
        command.arg("--token").arg(token);
    }
    let status = command
        .status()
        .await
        .with_context(|| format!("launch {} through {}", agent.label(), agent_bin.display()))?;
    if !status.success() {
        bail!("{} exited with {status}", agent_bin.display());
    }
    Ok(())
}

fn print_banner(session: &Value, agent: AgentKind, cwd: &Path) {
    println!("SessionWeft unified agent shell");
    println!("  session: {}", field_string(session, "id").unwrap_or("-"));
    println!(
        "  title:   {}",
        field_string(session, "title").unwrap_or("Untitled Session")
    );
    println!("  agent:   {}", agent.label());
    println!("  cwd:     {}", cwd.display());
    println!();
}

fn print_help() {
    println!("Slash commands:");
    println!("  /resume                 list every durable Session");
    println!("  /resume <SESSION_ID>    select the same Session for the current agent");
    println!("  /agent                  list supported adapters");
    println!("  /agent <AGENT>          switch adapter without changing Session");
    println!("  /new [TITLE]            create and select a new Session");
    println!("  /session                show the selected Session");
    println!("  /history                show shared cross-agent history");
    println!("  /context                show reconstructed shared context");
    println!("  /help                   show commands");
    println!("  /quit                   exit the shell");
    println!("Any other input is sent to the selected agent through the same Session.\n");
}

fn print_agents(active: AgentKind) {
    println!("SUPPORTED AGENTS");
    for agent in AgentKind::ALL {
        println!(
            "  {} {}",
            if agent == active { "*" } else { " " },
            agent.label()
        );
    }
    println!();
}

fn print_sessions(value: &Value) -> anyhow::Result<()> {
    let sessions = value
        .as_array()
        .context("Runtime Session list response is not an array")?;
    println!("SESSIONS ({})", sessions.len());
    println!("  {:<3} {:<36} {:>7} {:>8}  TITLE", "#", "ID", "VERSION", "MESSAGES");
    for (index, session) in sessions.iter().enumerate() {
        let id = field_string(session, "id").unwrap_or("-");
        let title = field_string(session, "title").unwrap_or("Untitled Session");
        let version = session.get("version").and_then(Value::as_u64).unwrap_or(0);
        let messages = session
            .get("messages")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        println!(
            "  {:<3} {:<36} {:>7} {:>8}  {}",
            index + 1,
            id,
            version,
            messages,
            title
        );
    }
    println!("\nSelect one with /resume <SESSION_ID>.\n");
    Ok(())
}

fn print_selected_session(session: &Value, agent: AgentKind) {
    println!("SESSION SELECTED");
    println!("  id:       {}", field_string(session, "id").unwrap_or("-"));
    println!(
        "  title:    {}",
        field_string(session, "title").unwrap_or("Untitled Session")
    );
    println!(
        "  version:  {}",
        session.get("version").and_then(Value::as_u64).unwrap_or(0)
    );
    println!(
        "  messages: {}",
        session
            .get("messages")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    );
    println!("  agent:    {}\n", agent.label());
}

fn print_history(session: &Value) -> anyhow::Result<()> {
    let messages = session
        .get("messages")
        .and_then(Value::as_array)
        .context("Session response is missing messages")?;
    println!(
        "HISTORY — {}",
        field_string(session, "title").unwrap_or("Untitled Session")
    );
    for message in messages {
        let role = field_string(message, "role").unwrap_or("unknown");
        let content = field_string(message, "content").unwrap_or_default();
        println!("\n[{role}]\n{content}");
    }
    println!();
    Ok(())
}

fn print_context(session: &Value, agent: AgentKind) -> anyhow::Result<()> {
    let messages = session
        .get("messages")
        .and_then(Value::as_array)
        .context("Session response is missing messages")?;
    println!("# SessionWeft shared context");
    println!("Session ID: {}", field_string(session, "id").unwrap_or("-"));
    println!(
        "Title: {}",
        field_string(session, "title").unwrap_or("Untitled Session")
    );
    println!("Current agent: {}", agent.label());
    println!("\n## Shared chat history");
    for message in messages.iter().rev().take(100).rev() {
        println!(
            "\n### {}\n{}",
            field_string(message, "role").unwrap_or("unknown"),
            field_string(message, "content").unwrap_or_default()
        );
    }
    println!();
    Ok(())
}

fn required_string(value: &Value, field: &'static str) -> anyhow::Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("Runtime response is missing string field {field}"))
}

fn field_string<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

fn short_id(session_id: &str) -> &str {
    session_id.get(..8).unwrap_or(session_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_slash_command_lists_or_selects_sessions() {
        assert_eq!(
            parse_slash_command("/resume"),
            Some(SlashCommand::Resume(None))
        );
        assert_eq!(
            parse_slash_command("/resume session-1"),
            Some(SlashCommand::Resume(Some("session-1".into())))
        );
        assert_eq!(
            parse_slash_command("/sessions"),
            Some(SlashCommand::Resume(None))
        );
    }

    #[test]
    fn all_supported_agent_aliases_resolve() {
        assert_eq!(AgentKind::parse("codex").unwrap(), AgentKind::Codex);
        assert_eq!(AgentKind::parse("claude-code").unwrap(), AgentKind::Claude);
        assert_eq!(AgentKind::parse("gemini_cli").unwrap(), AgentKind::Gemini);
        assert_eq!(AgentKind::parse("antigravity-ide").unwrap(), AgentKind::Antigravity);
        assert_eq!(AgentKind::parse("fcc-claude").unwrap(), AgentKind::FccClaude);
        assert_eq!(AgentKind::parse("fcc-codex").unwrap(), AgentKind::FccCodex);
    }

    #[test]
    fn regular_prompts_are_not_slash_commands() {
        assert_eq!(parse_slash_command("continue the implementation"), None);
    }
}
