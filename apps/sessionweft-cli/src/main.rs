use std::path::PathBuf;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

#[derive(Debug, Parser)]
#[command(name = "sessionweft", version, about = "SessionWeft Runtime CLI")]
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
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Health,
    Create {
        title: String,
    },
    List {
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    Get {
        id: String,
    },
    Message {
        id: String,
        expected_version: u64,
        content: String,
        #[arg(long, default_value = "user")]
        role: String,
    },
    Provider {
        id: String,
        expected_version: u64,
        provider: String,
        model: String,
    },
    Run {
        id: String,
        expected_version: u64,
        input: String,
    },
    Archive {
        id: String,
        expected_version: u64,
    },
    AgentRegister {
        session_id: String,
        #[arg(long)]
        manifest: PathBuf,
    },
    AgentGet {
        session_id: String,
        agent_id: String,
    },
    AgentStart {
        session_id: String,
        agent_id: String,
        expected_version: u64,
    },
    AgentHeartbeat {
        session_id: String,
        agent_id: String,
        expected_version: u64,
    },
    AgentStop {
        session_id: String,
        agent_id: String,
        expected_version: u64,
    },
    WorkflowCreate {
        session_id: String,
        #[arg(long)]
        definition: PathBuf,
    },
    WorkflowGet {
        session_id: String,
        workflow_id: String,
    },
    WorkflowNodeStart {
        session_id: String,
        workflow_id: String,
        node_id: String,
        expected_version: u64,
        owner_id: String,
    },
    WorkflowNodeComplete {
        session_id: String,
        workflow_id: String,
        node_id: String,
        expected_version: u64,
    },
    WorkflowNodeFail {
        session_id: String,
        workflow_id: String,
        node_id: String,
        expected_version: u64,
        error: String,
    },
    WorkflowApproval {
        session_id: String,
        workflow_id: String,
        node_id: String,
        expected_version: u64,
        approved: bool,
    },
    LockAcquire {
        session_id: String,
        #[arg(long)]
        request: PathBuf,
    },
    LockList {
        session_id: String,
        workspace_id: String,
    },
    LockHeartbeat {
        session_id: String,
        lock_id: String,
        workspace_id: String,
        owner_id: String,
        fencing_token: u64,
        ttl_seconds: u32,
    },
    LockRelease {
        session_id: String,
        lock_id: String,
        workspace_id: String,
        owner_id: String,
        fencing_token: u64,
    },
    MemoryPut {
        session_id: String,
        #[arg(long)]
        record: PathBuf,
    },
    MemorySearch {
        session_id: String,
        #[arg(long)]
        query: PathBuf,
    },
    MemoryForget {
        session_id: String,
        memory_id: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let client = reqwest::Client::new();
    let endpoint = cli.endpoint.trim_end_matches('/');

    let (method, path, body) = match cli.command {
        Command::Health => (Method::GET, "/health/ready".to_owned(), None),
        Command::Create { title } => (
            Method::POST,
            "/v1/sessions".to_owned(),
            Some(json!({"title": title})),
        ),
        Command::List { limit } => (Method::GET, format!("/v1/sessions?limit={limit}"), None),
        Command::Get { id } => (Method::GET, format!("/v1/sessions/{id}"), None),
        Command::Message {
            id,
            expected_version,
            content,
            role,
        } => (
            Method::POST,
            format!("/v1/sessions/{id}/messages"),
            Some(json!({
                "expected_version": expected_version,
                "role": role,
                "content": content,
            })),
        ),
        Command::Provider {
            id,
            expected_version,
            provider,
            model,
        } => (
            Method::POST,
            format!("/v1/sessions/{id}/provider"),
            Some(json!({
                "expected_version": expected_version,
                "provider": provider,
                "model": model,
            })),
        ),
        Command::Run {
            id,
            expected_version,
            input,
        } => (
            Method::POST,
            format!("/v1/sessions/{id}/run"),
            Some(json!({
                "expected_version": expected_version,
                "input": input,
            })),
        ),
        Command::Archive {
            id,
            expected_version,
        } => (
            Method::POST,
            format!("/v1/sessions/{id}/archive"),
            Some(json!({"expected_version": expected_version})),
        ),
        Command::AgentRegister {
            session_id,
            manifest,
        } => (
            Method::POST,
            format!("/v1/sessions/{session_id}/agents"),
            Some(read_json(manifest).await?),
        ),
        Command::AgentGet {
            session_id,
            agent_id,
        } => (
            Method::GET,
            format!("/v1/sessions/{session_id}/agents/{agent_id}"),
            None,
        ),
        Command::AgentStart {
            session_id,
            agent_id,
            expected_version,
        } => versioned_agent_request(session_id, agent_id, "start", expected_version),
        Command::AgentHeartbeat {
            session_id,
            agent_id,
            expected_version,
        } => versioned_agent_request(session_id, agent_id, "heartbeat", expected_version),
        Command::AgentStop {
            session_id,
            agent_id,
            expected_version,
        } => versioned_agent_request(session_id, agent_id, "stop", expected_version),
        Command::WorkflowCreate {
            session_id,
            definition,
        } => (
            Method::POST,
            format!("/v1/sessions/{session_id}/workflows"),
            Some(read_json(definition).await?),
        ),
        Command::WorkflowGet {
            session_id,
            workflow_id,
        } => (
            Method::GET,
            format!("/v1/sessions/{session_id}/workflows/{workflow_id}"),
            None,
        ),
        Command::WorkflowNodeStart {
            session_id,
            workflow_id,
            node_id,
            expected_version,
            owner_id,
        } => (
            Method::POST,
            workflow_node_path(&session_id, &workflow_id, &node_id, "start"),
            Some(json!({
                "expected_version": expected_version,
                "owner_id": owner_id,
            })),
        ),
        Command::WorkflowNodeComplete {
            session_id,
            workflow_id,
            node_id,
            expected_version,
        } => (
            Method::POST,
            workflow_node_path(&session_id, &workflow_id, &node_id, "complete"),
            Some(json!({"expected_version": expected_version})),
        ),
        Command::WorkflowNodeFail {
            session_id,
            workflow_id,
            node_id,
            expected_version,
            error,
        } => (
            Method::POST,
            workflow_node_path(&session_id, &workflow_id, &node_id, "fail"),
            Some(json!({
                "expected_version": expected_version,
                "error": error,
            })),
        ),
        Command::WorkflowApproval {
            session_id,
            workflow_id,
            node_id,
            expected_version,
            approved,
        } => (
            Method::POST,
            workflow_node_path(&session_id, &workflow_id, &node_id, "approval"),
            Some(json!({
                "expected_version": expected_version,
                "approved": approved,
            })),
        ),
        Command::LockAcquire {
            session_id,
            request,
        } => (
            Method::POST,
            format!("/v1/sessions/{session_id}/locks"),
            Some(read_json(request).await?),
        ),
        Command::LockList {
            session_id,
            workspace_id,
        } => (
            Method::GET,
            format!("/v1/sessions/{session_id}/locks?workspace_id={workspace_id}"),
            None,
        ),
        Command::LockHeartbeat {
            session_id,
            lock_id,
            workspace_id,
            owner_id,
            fencing_token,
            ttl_seconds,
        } => (
            Method::POST,
            format!("/v1/sessions/{session_id}/locks/{lock_id}/heartbeat"),
            Some(json!({
                "workspace_id": workspace_id,
                "owner_id": owner_id,
                "fencing_token": fencing_token,
                "ttl_seconds": ttl_seconds,
            })),
        ),
        Command::LockRelease {
            session_id,
            lock_id,
            workspace_id,
            owner_id,
            fencing_token,
        } => (
            Method::POST,
            format!("/v1/sessions/{session_id}/locks/{lock_id}/release"),
            Some(json!({
                "workspace_id": workspace_id,
                "owner_id": owner_id,
                "fencing_token": fencing_token,
            })),
        ),
        Command::MemoryPut { session_id, record } => (
            Method::POST,
            format!("/v1/sessions/{session_id}/memories"),
            Some(read_json(record).await?),
        ),
        Command::MemorySearch { session_id, query } => (
            Method::POST,
            format!("/v1/sessions/{session_id}/memories/search"),
            Some(read_json(query).await?),
        ),
        Command::MemoryForget {
            session_id,
            memory_id,
        } => (
            Method::POST,
            format!("/v1/sessions/{session_id}/memories/{memory_id}/forget"),
            None,
        ),
    };

    let mut request = client.request(method, format!("{endpoint}{path}"));
    if let Some(token) = cli.token.as_deref() {
        request = request.bearer_auth(token);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }

    let response = request.send().await.context("failed to reach Runtime")?;
    let status = response.status();
    let bytes = response.bytes().await.context("failed to read response")?;
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };

    if !status.is_success() {
        print_json(&value)?;
        bail!("Runtime request failed with HTTP {status}");
    }

    if status == StatusCode::NO_CONTENT {
        return Ok(());
    }
    print_json(&value)
}

fn versioned_agent_request(
    session_id: String,
    agent_id: String,
    operation: &str,
    expected_version: u64,
) -> (Method, String, Option<Value>) {
    (
        Method::POST,
        format!("/v1/sessions/{session_id}/agents/{agent_id}/{operation}"),
        Some(json!({"expected_version": expected_version})),
    )
}

fn workflow_node_path(
    session_id: &str,
    workflow_id: &str,
    node_id: &str,
    operation: &str,
) -> String {
    format!(
        "/v1/sessions/{session_id}/workflows/{workflow_id}/nodes/{node_id}/{operation}"
    )
}

async fn read_json(path: PathBuf) -> anyhow::Result<Value> {
    let bytes = tokio::fs::read(&path)
        .await
        .with_context(|| format!("failed to read JSON file {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid JSON in {}", path.display()))
}

fn print_json(value: &Value) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).context("failed to serialize output")?
    );
    Ok(())
}
