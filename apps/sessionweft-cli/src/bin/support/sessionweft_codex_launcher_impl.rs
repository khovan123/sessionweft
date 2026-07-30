use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
};

use anyhow::{Context, bail};
use chrono::Utc;
use clap::Parser;
use reqwest::Client;
use serde_json::{Value, json};

use crate::{
    codex_native_binding::{
        AGENT, BindingStore, NativeBinding, discover_new, find_by_id, snapshot, validate_uuid,
    },
    codex_session_picker::{PickerResult, pick_session},
};

#[derive(Debug, Parser)]
#[command(
    name = "sw-codex",
    version,
    about = "Select a SessionWeft Session, then open or resume native Codex"
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

    /// Link an existing Codex thread to the selected SessionWeft Session.
    #[arg(long)]
    native_session: Option<String>,

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

pub(super) fn run() -> anyhow::Result<()> {
    main()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cwd = fs::canonicalize(&cli.cwd)
        .with_context(|| format!("resolve wrapper working directory {}", cli.cwd.display()))?;
    let runtime = RuntimeClient::new(cli.endpoint, cli.token);
    let mut bindings = BindingStore::load(&cwd)?;

    let session = select_session(&runtime, &bindings, &cli).await?;
    let session_id = required_string(&session, "id")?;

    if let Some(native_id) = cli.native_session.as_deref() {
        validate_uuid(native_id)?;
        let now = Utc::now().to_rfc3339();
        bindings.upsert(NativeBinding {
            sessionweft_session_id: session_id.clone(),
            agent: AGENT.to_owned(),
            native_session_id: native_id.to_owned(),
            last_started_at: now.clone(),
            last_ended_at: now,
        })?;
    }

    let existing_binding = bindings.get(&session_id).cloned();
    print_session(&session, existing_binding.as_ref());
    materialize_context(&cwd, &session, existing_binding.as_ref())?;

    let before = snapshot(&cwd);
    let started_at = Utc::now();
    let status = launch_codex(
        &cwd,
        &cli.passthrough,
        existing_binding
            .as_ref()
            .map(|binding| binding.native_session_id.as_str()),
    )?;
    let ended_at = Utc::now();

    let record = existing_binding
        .as_ref()
        .and_then(|binding| find_by_id(&cwd, &binding.native_session_id))
        .or_else(|| discover_new(&cwd, &before));
    if let Some(record) = record {
        bindings.upsert(NativeBinding {
            sessionweft_session_id: session_id,
            agent: AGENT.to_owned(),
            native_session_id: record.id,
            last_started_at: started_at.to_rfc3339(),
            last_ended_at: ended_at.to_rfc3339(),
        })?;
    } else if existing_binding.is_none() {
        eprintln!(
            "warning: Codex session id could not be detected; run sw-codex --session <SESSIONWEFT_ID> --native-session <CODEX_ID> once to link it"
        );
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

fn launch_codex(
    cwd: &Path,
    args: &[OsString],
    native_session_id: Option<&str>,
) -> anyhow::Result<ExitStatus> {
    let mut command = Command::new("codex");
    if let Some(native_session_id) = native_session_id {
        command.arg("resume").arg(native_session_id);
    }
    command
        .args(args)
        .current_dir(cwd)
        .env("SESSIONWEFT_WRAPPED_AGENT", AGENT)
        .env(
            "SESSIONWEFT_CONTEXT_FILE",
            cwd.join(".sessionweft/active-context.md"),
        )
        .env(
            "SESSIONWEFT_SESSION_FILE",
            cwd.join(".sessionweft/active-session"),
        )
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("launch native Codex CLI")
}

fn materialize_context(
    cwd: &Path,
    session: &Value,
    binding: Option<&NativeBinding>,
) -> anyhow::Result<()> {
    let directory = cwd.join(".sessionweft");
    fs::create_dir_all(&directory)?;
    let id = required_string(session, "id")?;
    let title = session
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Untitled Session");
    let version = session
        .get("version")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let native_id = binding
        .map(|binding| binding.native_session_id.as_str())
        .unwrap_or("not-yet-bound");
    let mut content = format!(
        "# SessionWeft active context\n\n- Session ID: `{id}`\n- Title: {title}\n- Shared version: {version}\n- Agent: {AGENT}\n- Native Codex session: `{native_id}`\n\n## Shared history\n"
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
    fs::write(directory.join("active-context.md"), content)?;
    fs::write(directory.join("active-session"), id)?;
    Ok(())
}

fn print_session(session: &Value, binding: Option<&NativeBinding>) {
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
        session
            .get("version")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    );
    println!("  agent:          {AGENT}");
    println!(
        "  native session: {}",
        binding
            .map(|binding| binding.native_session_id.as_str())
            .unwrap_or("new session")
    );
    println!(
        "  last started:   {}",
        binding
            .map(|binding| binding.last_started_at.as_str())
            .unwrap_or("-")
    );
    println!(
        "  last ended:     {}",
        binding
            .map(|binding| binding.last_ended_at.as_str())
            .unwrap_or("-")
    );
    println!(
        "{}\n",
        if binding.is_some() {
            "Resuming native Codex CLI..."
        } else {
            "Opening new native Codex CLI..."
        }
    );
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
        bail!("native Codex exited with {status}");
    }
    Ok(())
}
