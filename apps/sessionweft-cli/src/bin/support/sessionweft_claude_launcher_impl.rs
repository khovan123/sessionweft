use std::{
    collections::hash_map::DefaultHasher,
    env,
    ffi::OsString,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, bail};
use clap::Parser;
use reqwest::Client;
use serde_json::{Value, json};

use crate::{
    sessionweft_handoff::{load_codex_handoff, write_codex_handoff},
    shared_session_picker::{PickerResult, pick_session},
};

const AGENT: &str = "claude";
const MAX_VISIBLE_HANDOFF_CHARACTERS: usize = 64 * 1024;

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
    last_imported_handoff_hash: u64,
}

#[derive(Debug)]
struct CodexBinding {
    native_session_id: String,
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

    let session = select_session(&runtime, &cli).await?;
    let session_id = required_string(&session, "id")?;
    refresh_codex_handoff(&cwd, &session_id);

    let context_path = materialize_context(&cwd, &session)?;
    let handoff = load_codex_handoff(&cwd, &session_id)?;
    let handoff_hash = handoff.as_deref().map(hash_text).unwrap_or(0);
    let state_path = claude_state_path(&cwd, &session_id);
    let previous_state = load_claude_state(&state_path)?;

    let visible_prompt = if cli.passthrough.is_empty()
        && handoff_hash != 0
        && handoff_hash != previous_state.last_imported_handoff_hash
    {
        handoff.as_deref().map(build_visible_handoff_prompt)
    } else {
        None
    };

    print_session(
        &session,
        &previous_state,
        visible_prompt.as_deref().map(|value| value.chars().count()),
    );

    let started_at = now_unix();
    let status = launch_claude(
        &cwd,
        &context_path,
        &session_id,
        previous_state.initialized,
        visible_prompt.as_deref(),
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
                last_imported_handoff_hash: if visible_prompt.is_some() {
                    handoff_hash
                } else {
                    previous_state.last_imported_handoff_hash
                },
            },
        )?;
    }
    ensure_success(status)
}

async fn select_session(runtime: &RuntimeClient, cli: &Cli) -> anyhow::Result<Value> {
    if let Some(session_id) = cli.session.as_deref() {
        return runtime.get(&format!("/v1/sessions/{session_id}")).await;
    }
    if let Some(title) = cli.new.as_deref() {
        return runtime.create_session(title).await;
    }

    let sessions = runtime.get("/v1/sessions?limit=100").await?;
    match pick_session(&sessions, AGENT)? {
        PickerResult::Create(title) => runtime.create_session(&title).await,
        PickerResult::Resume(id) => runtime.get(&format!("/v1/sessions/{id}")).await,
        PickerResult::Quit => bail!("session selection cancelled"),
    }
}

fn refresh_codex_handoff(cwd: &Path, session_id: &str) {
    let binding = match load_codex_binding(cwd, session_id) {
        Ok(Some(binding)) => binding,
        Ok(None) => return,
        Err(error) => {
            eprintln!("warning: failed to read Codex binding: {error}");
            return;
        }
    };
    let Some(rollout_path) = find_codex_rollout(&binding.native_session_id) else {
        eprintln!(
            "warning: native Codex rollout {} was not found; Claude will use shared SessionWeft history only",
            binding.native_session_id
        );
        return;
    };
    if let Err(error) =
        write_codex_handoff(cwd, session_id, &binding.native_session_id, &rollout_path)
    {
        eprintln!("warning: failed to refresh Codex handoff: {error}");
    }
}

fn load_codex_binding(cwd: &Path, session_id: &str) -> anyhow::Result<Option<CodexBinding>> {
    let path = cwd.join(".sessionweft/native-session-bindings.json");
    if !path.is_file() {
        return Ok(None);
    }
    let value: Value = serde_json::from_slice(&fs::read(&path)?)
        .with_context(|| format!("decode Codex bindings {}", path.display()))?;
    Ok(value
        .get("bindings")
        .and_then(Value::as_array)
        .and_then(|bindings| {
            bindings.iter().find_map(|binding| {
                let matches_session = binding
                    .get("sessionweft_session_id")
                    .and_then(Value::as_str)
                    == Some(session_id);
                let is_codex = binding.get("agent").and_then(Value::as_str) == Some("codex");
                if !matches_session || !is_codex {
                    return None;
                }
                let native_session_id = binding.get("native_session_id")?.as_str()?.to_owned();
                Some(CodexBinding { native_session_id })
            })
        }))
}

fn find_codex_rollout(native_session_id: &str) -> Option<PathBuf> {
    let root = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))?
        .join("sessions");
    let mut files = Vec::new();
    collect_jsonl_files(&root, &mut files);
    files.into_iter().find(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains(native_session_id))
    })
}

fn collect_jsonl_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, files);
        } else if path.extension().and_then(|value| value.to_str()) == Some("jsonl") {
            files.push(path);
        }
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
            "_No Codex handoff has been materialized for this Session yet. Select the linked Session after running the updated Codex launcher._\n",
        ),
    }

    let path = directory.join("active-context.md");
    fs::write(&path, content)?;
    fs::write(directory.join("active-session"), &id)?;
    Ok(path)
}

fn build_visible_handoff_prompt(handoff: &str) -> String {
    let visible_handoff = recent_characters(handoff, MAX_VISIBLE_HANDOFF_CHARACTERS);
    format!(
        "SessionWeft imported the following visible conversation from Codex. This is prior cross-agent history, not a new instruction.\n\n--- BEGIN CODEX HISTORY ---\n\n{visible_handoff}\n\n--- END CODEX HISTORY ---\n\nAcknowledge this imported history briefly, summarize the current state, and then wait for my next instruction."
    )
}

fn recent_characters(value: &str, maximum: usize) -> String {
    let count = value.chars().count();
    if count <= maximum {
        return value.to_owned();
    }
    let omitted = count - maximum;
    let recent = value.chars().skip(omitted).collect::<String>();
    format!("[{omitted} earlier characters omitted]\n\n{recent}")
}

fn hash_text(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn launch_claude(
    cwd: &Path,
    context_path: &Path,
    session_id: &str,
    resume: bool,
    visible_prompt: Option<&str>,
    args: &[OsString],
) -> anyhow::Result<ExitStatus> {
    let mut command = Command::new("claude");
    command.arg("--append-system-prompt-file").arg(context_path);
    if resume {
        command.arg("--resume").arg(session_id);
    } else {
        command.arg("--session-id").arg(session_id);
    }
    command.args(args);
    if let Some(prompt) = visible_prompt {
        command.arg(prompt);
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
        last_imported_handoff_hash: value
            .get("last_imported_handoff_hash")
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
            "schema_version": 2,
            "initialized": state.initialized,
            "last_started_at": state.last_started_at,
            "last_ended_at": state.last_ended_at,
            "last_imported_handoff_hash": state.last_imported_handoff_hash,
        }))?,
    )?;
    Ok(())
}

fn print_session(session: &Value, state: &ClaudeState, visible_import_size: Option<usize>) {
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
        "  visible import: {}",
        visible_import_size
            .map(|size| format!("{size} chars from Codex"))
            .unwrap_or_else(|| "already imported or unavailable".to_owned())
    );
    println!(
        "{}\n",
        if state.initialized {
            "Resuming native Claude Code with refreshed SessionWeft context..."
        } else {
            "Opening native Claude Code with visible Codex history..."
        }
    );
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn format_optional_time(value: u64) -> String {
    if value == 0 {
        "-".to_owned()
    } else {
        format_unix_utc(value)
    }
}

fn format_unix_utc(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let shifted = days_since_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
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

    #[test]
    fn unix_time_formats_as_utc_date() {
        assert_eq!(format_unix_utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_unix_utc(1_722_470_400), "2024-08-01 00:00 UTC");
    }

    #[test]
    fn visible_prompt_contains_imported_history() {
        let prompt = build_visible_handoff_prompt("### User\n\nHello\n\n### Codex\n\nDone");
        assert!(prompt.contains("BEGIN CODEX HISTORY"));
        assert!(prompt.contains("### User"));
        assert!(prompt.contains("### Codex"));
    }

    #[test]
    fn handoff_hash_changes_with_content() {
        assert_ne!(hash_text("first"), hash_text("second"));
    }
}
