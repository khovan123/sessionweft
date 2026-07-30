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

use anyhow::{Context, bail, ensure};
use clap::Parser;
use reqwest::Client;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    sessionweft_handoff::{load_codex_handoff, write_codex_handoff},
    shared_session_picker::{PickerResult, pick_session},
};

const AGENT: &str = "claude";
const IMPORTED_ASSISTANT_MODEL: &str = "sessionweft-imported-codex";

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
    native_session_id: Option<String>,
    imported_handoff_hash: u64,
    last_started_at: u64,
    last_ended_at: u64,
}

#[derive(Debug)]
struct CodexBinding {
    native_session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportedRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImportedMessage {
    role: ImportedRole,
    text: String,
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
    let sessionweft_session_id = required_string(&session, "id")?;
    refresh_codex_handoff(&cwd, &sessionweft_session_id);

    let handoff = load_codex_handoff(&cwd, &sessionweft_session_id)?
        .with_context(|| format!("Session {sessionweft_session_id} has no Codex handoff to import"))?;
    let imported_messages = parse_handoff_messages(&handoff);
    ensure!(
        !imported_messages.is_empty(),
        "Codex handoff contains no visible user or assistant messages"
    );

    let handoff_hash = hash_text(&handoff);
    let state_path = claude_state_path(&cwd, &sessionweft_session_id);
    let mut state = load_claude_state(&state_path)?;

    let needs_native_import = state.imported_handoff_hash != handoff_hash
        || state
            .native_session_id
            .as_deref()
            .is_none_or(|id| !claude_transcript_path(&cwd, id).is_file());

    if needs_native_import {
        let previous_native_session_id = state.native_session_id.take();
        let native_session_id = Uuid::new_v4().to_string();
        let transcript_path =
            write_native_claude_transcript(&cwd, &native_session_id, &imported_messages)?;
        state.native_session_id = Some(native_session_id.clone());
        state.imported_handoff_hash = handoff_hash;
        save_claude_state(&state_path, &state)?;

        println!(
            "SessionWeft created native Claude history: {}",
            transcript_path.display()
        );
        println!(
            "Imported {} Codex turns as Claude UI history.",
            imported_messages.len()
        );
        if let Some(previous) = previous_native_session_id {
            println!("Previous SessionWeft Claude session preserved: {previous}");
        }
    }

    let native_session_id = state
        .native_session_id
        .clone()
        .context("native Claude session id was not persisted")?;
    let context_path = materialize_context(&cwd, &session, &native_session_id)?;

    print_session(
        &session,
        &state,
        &native_session_id,
        imported_messages.len(),
    );
    state.last_started_at = now_unix();
    save_claude_state(&state_path, &state)?;

    let status = launch_claude(
        &cwd,
        &context_path,
        &native_session_id,
        &cli.passthrough,
    )?;
    state.last_ended_at = now_unix();
    save_claude_state(&state_path, &state)?;
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
            "warning: native Codex rollout {} was not found",
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
                Some(CodexBinding {
                    native_session_id: binding.get("native_session_id")?.as_str()?.to_owned(),
                })
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

fn parse_handoff_messages(handoff: &str) -> Vec<ImportedMessage> {
    let mut messages = Vec::new();
    let mut role = None;
    let mut body = String::new();
    let mut in_visible_conversation = false;

    for line in handoff.lines() {
        if line.trim() == "## Visible conversation" {
            in_visible_conversation = true;
            continue;
        }
        if !in_visible_conversation {
            continue;
        }

        let next_role = match line.trim() {
            "### User" => Some(ImportedRole::User),
            "### Codex" => Some(ImportedRole::Assistant),
            _ => None,
        };
        if let Some(next_role) = next_role {
            push_imported_message(&mut messages, role, &mut body);
            role = Some(next_role);
        } else if role.is_some() {
            body.push_str(line);
            body.push('\n');
        }
    }
    push_imported_message(&mut messages, role, &mut body);
    messages
}

fn push_imported_message(
    messages: &mut Vec<ImportedMessage>,
    role: Option<ImportedRole>,
    body: &mut String,
) {
    let Some(role) = role else {
        body.clear();
        return;
    };
    let text = body.trim().to_owned();
    body.clear();
    if !text.is_empty() {
        messages.push(ImportedMessage { role, text });
    }
}

fn write_native_claude_transcript(
    cwd: &Path,
    native_session_id: &str,
    messages: &[ImportedMessage],
) -> anyhow::Result<PathBuf> {
    let path = claude_transcript_path(cwd, native_session_id);
    let parent = path
        .parent()
        .context("Claude transcript path has no parent directory")?;
    fs::create_dir_all(parent)?;

    let version = claude_version();
    let git_branch = git_branch(cwd);
    let cwd_text = cwd.to_string_lossy();
    let started = now_unix().saturating_sub(messages.len() as u64);
    let mut previous_uuid: Option<String> = None;
    let mut content = String::new();

    for (index, imported) in messages.iter().enumerate() {
        let uuid = Uuid::new_v4();
        let uuid_text = uuid.to_string();
        let timestamp = format_rfc3339_utc(started.saturating_add(index as u64));
        let record = match imported.role {
            ImportedRole::User => json!({
                "parentUuid": previous_uuid,
                "isSidechain": false,
                "userType": "external",
                "cwd": cwd_text,
                "sessionId": native_session_id,
                "version": version,
                "gitBranch": git_branch,
                "type": "user",
                "message": {
                    "role": "user",
                    "content": imported.text,
                },
                "uuid": uuid_text,
                "timestamp": timestamp,
                "sessionweftImported": true,
                "sourceAgent": "codex",
            }),
            ImportedRole::Assistant => json!({
                "parentUuid": previous_uuid,
                "isSidechain": false,
                "userType": "external",
                "cwd": cwd_text,
                "sessionId": native_session_id,
                "version": version,
                "gitBranch": git_branch,
                "message": {
                    "model": IMPORTED_ASSISTANT_MODEL,
                    "id": format!("msg_sessionweft_{}", uuid.simple()),
                    "type": "message",
                    "role": "assistant",
                    "content": [{
                        "type": "text",
                        "text": format!("[Imported from Codex]\n\n{}", imported.text),
                    }],
                    "stop_reason": "end_turn",
                    "stop_sequence": null,
                    "usage": {
                        "input_tokens": 0,
                        "cache_creation_input_tokens": 0,
                        "cache_read_input_tokens": 0,
                        "output_tokens": 0,
                        "service_tier": "standard",
                    },
                },
                "requestId": format!("sessionweft-import-{}", uuid.simple()),
                "type": "assistant",
                "uuid": uuid_text,
                "timestamp": timestamp,
                "sessionweftImported": true,
                "sourceAgent": "codex",
            }),
        };
        content.push_str(&serde_json::to_string(&record)?);
        content.push('\n');
        previous_uuid = Some(uuid_text);
    }

    validate_parent_chain(&content)?;
    let temporary_path = path.with_extension("jsonl.sessionweft.tmp");
    fs::write(&temporary_path, content)?;
    fs::rename(&temporary_path, &path)?;
    Ok(path)
}

fn validate_parent_chain(content: &str) -> anyhow::Result<()> {
    let mut previous_uuid: Option<String> = None;
    for (index, line) in content.lines().enumerate() {
        let value: Value = serde_json::from_str(line)
            .with_context(|| format!("decode generated Claude JSONL line {}", index + 1))?;
        let parent_uuid = value.get("parentUuid").and_then(Value::as_str);
        ensure!(
            parent_uuid == previous_uuid.as_deref(),
            "generated Claude parent chain broke at line {}",
            index + 1
        );
        previous_uuid = Some(
            value
                .get("uuid")
                .and_then(Value::as_str)
                .context("generated Claude message is missing uuid")?
                .to_owned(),
        );
    }
    Ok(())
}

fn claude_transcript_path(cwd: &Path, native_session_id: &str) -> PathBuf {
    claude_config_root()
        .join("projects")
        .join(encode_project_path(cwd))
        .join(format!("{native_session_id}.jsonl"))
}

fn claude_config_root() -> PathBuf {
    if let Some(directory) = env::var_os("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(directory);
    }
    let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let standard = home.join(".claude");
    let config = home.join(".config/claude");
    if standard.exists() || !config.exists() {
        standard
    } else {
        config
    }
}

fn encode_project_path(cwd: &Path) -> String {
    cwd.to_string_lossy().replace('/', "-").replace('\\', "-")
}

fn claude_version() -> String {
    Command::new("claude")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|output| output.split_whitespace().next().map(str::to_owned))
        .unwrap_or_else(|| "sessionweft-import".to_owned())
}

fn git_branch(cwd: &Path) -> String {
    Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|branch| branch.trim().to_owned())
        .unwrap_or_default()
}

fn materialize_context(
    cwd: &Path,
    session: &Value,
    native_session_id: &str,
) -> anyhow::Result<PathBuf> {
    let directory = cwd.join(".sessionweft");
    fs::create_dir_all(&directory)?;
    let session_id = required_string(session, "id")?;
    let title = session
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Untitled Session");
    let content = format!(
        "# SessionWeft native Claude continuation\n\n- SessionWeft Session: `{session_id}`\n- Native Claude Session: `{native_session_id}`\n- Title: {title}\n\nThe visible earlier user and assistant turns were imported from Codex by SessionWeft. Assistant entries marked `[Imported from Codex]` are historical Codex outputs, not claims that Claude generated them. Continue from that history without repeating it.\n"
    );
    let path = directory.join("active-context.md");
    fs::write(&path, content)?;
    fs::write(directory.join("active-session"), session_id)?;
    Ok(path)
}

fn launch_claude(
    cwd: &Path,
    context_path: &Path,
    native_session_id: &str,
    args: &[OsString],
) -> anyhow::Result<ExitStatus> {
    Command::new("claude")
        .arg("--append-system-prompt-file")
        .arg(context_path)
        .arg("--resume")
        .arg(native_session_id)
        .args(args)
        .current_dir(cwd)
        .env("SESSIONWEFT_WRAPPED_AGENT", AGENT)
        .env("SESSIONWEFT_CONTEXT_FILE", context_path)
        .env("SESSIONWEFT_SESSION_ID", native_session_id)
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
        native_session_id: value
            .get("native_session_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        imported_handoff_hash: value
            .get("imported_handoff_hash")
            .or_else(|| value.get("last_imported_handoff_hash"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
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
            "schema_version": 3,
            "native_session_id": state.native_session_id,
            "imported_handoff_hash": state.imported_handoff_hash,
            "last_started_at": state.last_started_at,
            "last_ended_at": state.last_ended_at,
        }))?,
    )?;
    Ok(())
}

fn print_session(
    session: &Value,
    state: &ClaudeState,
    native_session_id: &str,
    imported_turns: usize,
) {
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
    println!("  agent:          {AGENT}");
    println!("  native session: {native_session_id}");
    println!("  imported turns: {imported_turns}");
    println!(
        "  last started:   {}",
        format_optional_time(state.last_started_at)
    );
    println!(
        "  last ended:     {}",
        format_optional_time(state.last_ended_at)
    );
    println!("Resuming native Claude Code with imported bubble history...\n");
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

fn format_rfc3339_utc(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.000Z")
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

fn hash_text(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
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
    fn parses_handoff_into_native_turns() {
        let messages = parse_handoff_messages(
            "# Header\n\n## Visible conversation\n\n### User\n\nhi\n\n### Codex\n\nHi.\n",
        );
        assert_eq!(
            messages,
            vec![
                ImportedMessage {
                    role: ImportedRole::User,
                    text: "hi".to_owned(),
                },
                ImportedMessage {
                    role: ImportedRole::Assistant,
                    text: "Hi.".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn validates_linear_parent_chain() {
        let first = Uuid::new_v4().to_string();
        let second = Uuid::new_v4().to_string();
        let content = format!(
            "{}\n{}\n",
            json!({"parentUuid": null, "uuid": first}),
            json!({"parentUuid": first, "uuid": second})
        );
        validate_parent_chain(&content).unwrap();
    }

    #[test]
    fn encodes_project_path_like_claude() {
        assert_eq!(
            encode_project_path(Path::new("/home/khovan/sessionweft")),
            "-home-khovan-sessionweft"
        );
    }

    #[test]
    fn unix_time_formats_as_utc_date() {
        assert_eq!(format_unix_utc(1_722_470_400), "2024-08-01 00:00 UTC");
    }
}
