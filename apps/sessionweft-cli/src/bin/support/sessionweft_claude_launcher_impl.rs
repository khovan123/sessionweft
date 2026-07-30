use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
};

use anyhow::{Context, bail};
use clap::Parser;
use reqwest::Client;
use serde_json::{Value, json};

use crate::{
    codex_native_binding::{BindingStore, find_by_id, format_unix_utc, now_unix},
    codex_session_picker::{PickerResult, pick_session},
    sessionweft_handoff::{load_codex_handoff, write_codex_handoff},
};

const AGENT: &str = "claude";

#[derive(Debug, Parser)]
#[command(
    name = "sw-claude",
    version,
    about = "Continue a SessionWeft Session in native Claude Code"
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

    #[arg(last = true)]
    passthrough: Vec<OsString>,
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
    let value = response
        .json::<Value>()
        .await
        .context("decode Runtime response")?;
    if !status.is_success() {
        bail!("SessionWeft Runtime returned HTTP {status}: {value}");
    }
    Ok(value)
}

#[derive(Debug, Default)]
struct ClaudeState {
    initialized: bool,
    last_started_at: u64,
    last_ended_at: u64,
}

pub(super) fn run() -> anyhow::Result<()> {
    main()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cwd = fs::canonicalize(&cli.cwd)
        .with_context(|| format!("resolve wrapper working directory {}", cli.cwd.display()))?;
    let runtime = RuntimeClient::new(cli.endpoint.clone(), cli.token.clone());
    let bindings = BindingStore::load(&cwd)?;

    let session = select_session(&runtime, &bindings, &cli).await?;
    let session_id = required_string(&session, "id")?;
    refresh_codex_handoff(&cwd, &session_id, &bindings);
    let context_path = materialize_context(&cwd, &session)?;
    let state_path = claude_state_path(&cwd, &session_id);
    let previous_state = load_claude_state(&state_path)?;

    print_session(&session, &previous_state);
    let started_at = now_unix();
    let status = launch_claude(
        &cwd,
        &context_path,
        &session_id,
        previous_state.initialized,
        &cli.passthrough,
    )?;
    let ended_at = now_unix();

    if status.success() {
        save_claude_state(
            &state_path,
            &ClaudeState {
                initialized: true,
                last_started_at: started_at,
                last_ended_at: ended_at,
            },
        )?;
    }
    ensure_success(status)
}

async fn select_session(
    runtime: &RuntimeClient,
    bindings: &BindingStore,
    cli: &Cli,
) -> anyhow::Result<Value> {
    if let Some(session_id) = cli.session.as_deref() {
        return runtime.get(&format!("/v1/sessions/{session_id}")).await;
    }
    if let Some(title) = cli.new.as_deref() {
        return runtime.create_session(title).await;
    }

    let sessions = runtime.get("/v1/sessions?limit=100").await?;
    match pick_session(&sessions, bindings)? {
        PickerResult::Create(title) => runtime.create_session(&title).await,
        PickerResult::Resume(id) => runtime.get(&format!("/v1/sessions/{id}")).await,
        PickerResult::Quit => bail!("session selection cancelled"),
    }
}

fn refresh_codex_handoff(cwd: &Path, session_id: &str, bindings: &BindingStore) {
    let Some(binding) = bindings.get(session_id) else {
        return;
    };
    let Some(record) = find_by_id(cwd, &binding.native_session_id) else {
        eprintln!(
            "warning: native Codex rollout {} was not found; Claude will use shared SessionWeft history only",
            binding.native_session_id
        );
        return;
    };
    if let Err(error) = write_codex_handoff(cwd, session_id, &record.id, &record.path) {
        eprintln!("warning: failed to refresh Codex handoff: {error}");
    }
}

fn materialize_context(cwd: &Path, session: &Value) -> anyhow::Result<PathBuf> {
    let directory = cwd.join(".sessionweft");
    fs::create_dir_all(&directory)?;
    let id = required_string(session, "id")?;
    let title = session
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Untitled Session");
    let version = session.get("version").and_then(Value::as_u64).unwrap_or(0);
    let mut content = format!(
        "# SessionWeft active context\n\n- Session ID: `{id}`\n- Title: {title}\n- Shared version: {version}\n- Agent: {AGENT}\n\n## Continuation instructions\n\nTreat the shared history and Codex handoff below as the prior conversation for this task. Preserve decisions, completed work, unresolved issues, file paths, and user intent. Do not claim that Codex messages are native Claude messages.\n\n## Shared SessionWeft history\n"
    );
    if let Some(messages) = session.get("messages").and_then(Value::as_array) {
        for message in messages.iter().rev().take(200).rev() {
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let body = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            content.push_str(&format!("\n### {role}\n\n{body}\n"));
        }
    }

    content.push_str("\n## Codex cross-agent handoff\n\n");
    match load_codex_handoff(cwd, &id)? {
        Some(handoff) => content.push_str(&handoff),
        None => content.push_str(
            "_No Codex handoff has been materialized for this Session yet. Run the updated sw-codex launcher for this Session, or link its native Codex ID._\n",
        ),
    }

    let path = directory.join("active-context.md");
    fs::write(&path, content)?;
    fs::write(directory.join("active-session"), &id)?;
    Ok(path)
}

fn launch_claude(
    cwd: &Path,
    context_path: &Path,
    session_id: &str,
    resume: bool,
    args: &[OsString],
) -> anyhow::Result<ExitStatus> {
    let mut command = Command::new("claude");
    if resume {
        command.arg("--resume").arg(session_id);
    } else {
        command.arg("--session-id").arg(session_id);
    }
    command
        .arg("--append-system-prompt-file")
        .arg(context_path)
        .args(args);
    if !resume && args.is_empty() {
        command.arg(
            "Continue from the SessionWeft cross-agent handoff. Briefly summarize the previous Codex work and current state, then wait for my next instruction.",
        );
    }
    command
        .current_dir(cwd)
        .env("SESSIONWEFT_WRAPPED_AGENT", AGENT)
        .env("SESSIONWEFT_CONTEXT_FILE", context_path)
        .env(
            "SESSIONWEFT_SESSION_FILE",
            cwd.join(".sessionweft/active-session"),
        )
        .env("SESSIONWEFT_SESSION_ID", session_id)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("launch native Claude Code CLI")
}

fn claude_state_path(cwd: &Path, session_id: &str) -> PathBuf {
    cwd.join(".sessionweft")
        .join("claude-native-sessions")
        .join(format!("{session_id}.json"))
}

fn load_claude_state(path: &Path) -> anyhow::Result<ClaudeState> {
    if !path.is_file() {
        return Ok(ClaudeState::default());
    }
    let value: Value = serde_json::from_slice(&fs::read(path)?)
        .with_context(|| format!("decode Claude state {}", path.display()))?;
    Ok(ClaudeState {
        initialized: value
            .get("initialized")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        last_started_at: value
            .get("last_started_at")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        last_ended_at: value
            .get("last_ended_at")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    })
}

fn save_claude_state(path: &Path, state: &ClaudeState) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "initialized": state.initialized,
            "last_started_at": state.last_started_at,
            "last_ended_at": state.last_ended_at,
        }))?,
    )?;
    Ok(())
}

fn print_session(session: &Value, state: &ClaudeState) {
    println!("SESSION");
    println!(
        "  id:             {}",
        session.get("id").and_then(Value::as_str).unwrap_or("-")
    );
    println!(
        "  title:          {}",
        session
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Untitled Session")
    );
    println!(
        "  shared version: {}",
        session.get("version").and_then(Value::as_u64).unwrap_or(0)
    );
    println!("  agent:          {AGENT}");
    println!(
        "  native session: {}",
        session.get("id").and_then(Value::as_str).unwrap_or("-")
    );
    println!(
        "  last started:   {}",
        format_optional_time(state.last_started_at)
    );
    println!(
        "  last ended:     {}",
        format_optional_time(state.last_ended_at)
    );
    println!(
        "{}\n",
        if state.initialized {
            "Resuming native Claude Code with refreshed SessionWeft context..."
        } else {
            "Opening new native Claude Code from the Codex handoff..."
        }
    );
}

fn format_optional_time(value: u64) -> String {
    if value == 0 {
        "-".to_owned()
    } else {
        format_unix_utc(value)
    }
}

fn required_string(value: &Value, field: &str) -> anyhow::Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("Runtime response is missing '{field}'"))
}

fn ensure_success(status: ExitStatus) -> anyhow::Result<()> {
    if !status.success() {
        bail!("native Claude Code exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_time_is_not_rendered_as_epoch() {
        assert_eq!(format_optional_time(0), "-");
    }
}
