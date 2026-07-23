use std::{env, path::{Path, PathBuf}, process::Stdio};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand, ValueEnum};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use tokio::{fs, io::AsyncWriteExt, process::Command};

#[derive(Debug, Parser)]
#[command(
    name = "sessionweft-agent",
    version,
    about = "Run independent coding agents against durable SessionWeft sessions"
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

    #[command(subcommand)]
    command: AgentCommand,
}

#[derive(Debug, Subcommand)]
enum AgentCommand {
    /// Create a new durable chat session.
    New {
        title: String,
    },
    /// List sessions that can be resumed.
    Sessions {
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    /// Print the shared chat history for one session.
    History {
        session_id: String,
    },
    /// Build and print the exact context that would be sent to an agent.
    Context {
        session_id: String,
        #[arg(long, default_value_t = 100)]
        messages: usize,
    },
    /// Run or resume one agent on a durable shared session.
    Run {
        #[arg(value_enum)]
        agent: AgentKind,
        prompt: String,
        /// Existing Session ID. Omit it to create a new session.
        #[arg(long)]
        session: Option<String>,
        /// Title used only when a new session is created.
        #[arg(long, default_value = "Standalone agent session")]
        title: String,
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
        /// Maximum number of most-recent Session messages passed as context.
        #[arg(long, default_value_t = 100)]
        context_messages: usize,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AgentKind {
    Codex,
    Claude,
    Gemini,
    Antigravity,
}

impl AgentKind {
    fn label(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::Antigravity => "antigravity",
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

    async fn request(&self, method: Method, path: &str, body: Option<Value>) -> anyhow::Result<Value> {
        let mut request = self.http.request(method, format!("{}{}", self.endpoint, path));
        if let Some(token) = self.token.as_deref() {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }

        let response = request.send().await.context("failed to reach SessionWeft Runtime")?;
        let status = response.status();
        let bytes = response.bytes().await.context("failed to read Runtime response")?;
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        if !status.is_success() {
            eprintln!("{}", serde_json::to_string_pretty(&value)?);
            bail!("Runtime request failed with HTTP {status}");
        }
        if status == StatusCode::NO_CONTENT {
            return Ok(Value::Null);
        }
        Ok(value)
    }

    async fn create_session(&self, title: &str) -> anyhow::Result<Value> {
        self.request(Method::POST, "/v1/sessions", Some(json!({"title": title})))
            .await
    }

    async fn list_sessions(&self, limit: u32) -> anyhow::Result<Value> {
        self.request(Method::GET, &format!("/v1/sessions?limit={limit}"), None)
            .await
    }

    async fn get_session(&self, session_id: &str) -> anyhow::Result<Value> {
        self.request(Method::GET, &format!("/v1/sessions/{session_id}"), None)
            .await
    }

    async fn append_message(
        &self,
        session_id: &str,
        expected_version: u64,
        role: &str,
        content: &str,
    ) -> anyhow::Result<Value> {
        self.request(
            Method::POST,
            &format!("/v1/sessions/{session_id}/messages"),
            Some(json!({
                "expected_version": expected_version,
                "role": role,
                "content": content,
            })),
        )
        .await
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let runtime = RuntimeClient::new(cli.endpoint, cli.token);

    match cli.command {
        AgentCommand::New { title } => print_json(&runtime.create_session(&title).await?)?,
        AgentCommand::Sessions { limit } => print_json(&runtime.list_sessions(limit).await?)?,
        AgentCommand::History { session_id } => {
            let session = runtime.get_session(&session_id).await?;
            print_history(&session)?;
        }
        AgentCommand::Context { session_id, messages } => {
            let session = runtime.get_session(&session_id).await?;
            println!("{}", build_context(&session, messages)?);
        }
        AgentCommand::Run {
            agent,
            prompt,
            session,
            title,
            cwd,
            context_messages,
        } => {
            run_agent(&runtime, agent, session, &title, &cwd, &prompt, context_messages).await?;
        }
    }

    Ok(())
}

async fn run_agent(
    runtime: &RuntimeClient,
    agent: AgentKind,
    session_id: Option<String>,
    title: &str,
    cwd: &Path,
    prompt: &str,
    context_messages: usize,
) -> anyhow::Result<()> {
    let cwd = fs::canonicalize(cwd)
        .await
        .with_context(|| format!("failed to resolve working directory {}", cwd.display()))?;

    let initial = match session_id {
        Some(id) => runtime.get_session(&id).await?,
        None => runtime.create_session(title).await?,
    };
    let session_id = required_string(&initial, "id")?;
    let version = required_u64(&initial, "version")?;

    let tagged_prompt = format!("[agent:{}] {prompt}", agent.label());
    runtime
        .append_message(&session_id, version, "user", &tagged_prompt)
        .await?;

    let session = runtime.get_session(&session_id).await?;
    let context = build_context(&session, context_messages)?;

    if matches!(agent, AgentKind::Antigravity) {
        let context_path = write_antigravity_context(&cwd, &session_id, &context).await?;
        launch_antigravity(&cwd, &context_path).await?;
        let latest = runtime.get_session(&session_id).await?;
        let latest_version = required_u64(&latest, "version")?;
        let message = format!(
            "[agent:antigravity] IDE launched for {}. Shared Session context: {}",
            cwd.display(),
            context_path.display()
        );
        runtime
            .append_message(&session_id, latest_version, "assistant", &message)
            .await?;
        println!("session_id={session_id}");
        println!("agent=antigravity");
        println!("context_file={}", context_path.display());
        return Ok(());
    }

    let output = execute_terminal_agent(agent, &cwd, &context).await?;
    let latest = runtime.get_session(&session_id).await?;
    let latest_version = required_u64(&latest, "version")?;
    let tagged_output = format!("[agent:{}]\n{}", agent.label(), output.trim());
    let updated = runtime
        .append_message(&session_id, latest_version, "assistant", &tagged_output)
        .await?;

    println!("session_id={session_id}");
    println!("agent={}", agent.label());
    println!("response:\n{}", output.trim());
    println!("session_version={}", required_u64(&updated, "version")?);
    Ok(())
}

async fn execute_terminal_agent(agent: AgentKind, cwd: &Path, context: &str) -> anyhow::Result<String> {
    let (program, args, stdin_prompt) = match agent {
        AgentKind::Codex => (
            env::var("SESSIONWEFT_CODEX_BIN").unwrap_or_else(|_| "codex".into()),
            vec!["exec".to_owned(), "--skip-git-repo-check".to_owned(), "-".to_owned()],
            true,
        ),
        AgentKind::Claude => (
            env::var("SESSIONWEFT_CLAUDE_BIN").unwrap_or_else(|_| "claude".into()),
            vec!["-p".to_owned(), context.to_owned()],
            false,
        ),
        AgentKind::Gemini => (
            env::var("SESSIONWEFT_GEMINI_BIN").unwrap_or_else(|_| "gemini".into()),
            vec!["-p".to_owned(), context.to_owned()],
            false,
        ),
        AgentKind::Antigravity => unreachable!("Antigravity is launched as an IDE"),
    };

    let mut command = Command::new(&program);
    command
        .args(&args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin_prompt {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to launch {program}; install it or configure its SESSIONWEFT_*_BIN variable"))?;
    if stdin_prompt {
        let mut stdin = child.stdin.take().context("agent stdin was not available")?;
        stdin.write_all(context.as_bytes()).await?;
        stdin.shutdown().await?;
    }

    let output = child.wait_with_output().await.context("failed while waiting for agent")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{program} exited with {}: {}", output.status, stderr.trim());
    }
    let stdout = String::from_utf8(output.stdout).context("agent output was not valid UTF-8")?;
    if stdout.trim().is_empty() {
        bail!("{program} returned an empty response");
    }
    Ok(stdout)
}

async fn write_antigravity_context(cwd: &Path, session_id: &str, context: &str) -> anyhow::Result<PathBuf> {
    let directory = cwd.join(".sessionweft").join("contexts");
    fs::create_dir_all(&directory).await?;
    let path = directory.join(format!("{session_id}.md"));
    fs::write(&path, context).await?;
    Ok(path)
}

async fn launch_antigravity(cwd: &Path, context_path: &Path) -> anyhow::Result<()> {
    let program = env::var("SESSIONWEFT_ANTIGRAVITY_BIN")
        .unwrap_or_else(|_| "antigravity-ide".into());
    Command::new(&program)
        .arg(cwd)
        .env("SESSIONWEFT_SESSION_CONTEXT", context_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to launch {program}; configure SESSIONWEFT_ANTIGRAVITY_BIN if needed"))?;
    Ok(())
}

fn build_context(session: &Value, limit: usize) -> anyhow::Result<String> {
    let id = required_string(session, "id")?;
    let title = required_string(session, "title")?;
    let messages = session
        .get("messages")
        .and_then(Value::as_array)
        .context("Session response is missing messages")?;
    let start = messages.len().saturating_sub(limit);

    let mut result = format!(
        "# SessionWeft shared session\n\nSession ID: {id}\nTitle: {title}\n\n## Shared chat history\n"
    );
    for message in &messages[start..] {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("unknown");
        let content = message.get("content").and_then(Value::as_str).unwrap_or_default();
        result.push_str(&format!("\n### {role}\n{content}\n"));
    }
    result.push_str(
        "\n## Instructions\nContinue from this shared history. Preserve decisions and constraints from earlier agents. Answer only the latest user request.\n",
    );
    Ok(result)
}

fn print_history(session: &Value) -> anyhow::Result<()> {
    println!("Session: {} — {}", required_string(session, "id")?, required_string(session, "title")?);
    let messages = session
        .get("messages")
        .and_then(Value::as_array)
        .context("Session response is missing messages")?;
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("unknown");
        let content = message.get("content").and_then(Value::as_str).unwrap_or_default();
        println!("\n[{role}]\n{content}");
    }
    Ok(())
}

fn required_string(value: &Value, field: &'static str) -> anyhow::Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("Runtime response is missing string field {field}"))
}

fn required_u64(value: &Value, field: &'static str) -> anyhow::Result<u64> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .with_context(|| format!("Runtime response is missing integer field {field}"))
}

fn print_json(value: &Value) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_keeps_recent_messages_and_agent_provenance() {
        let session = json!({
            "id": "session-1",
            "title": "demo",
            "messages": [
                {"role": "user", "content": "old"},
                {"role": "assistant", "content": "[agent:codex] first"},
                {"role": "user", "content": "[agent:claude] continue"}
            ]
        });
        let context = build_context(&session, 2).expect("context");
        assert!(!context.contains("old"));
        assert!(context.contains("[agent:codex] first"));
        assert!(context.contains("[agent:claude] continue"));
    }
}
