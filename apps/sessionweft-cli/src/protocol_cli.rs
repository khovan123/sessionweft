use std::time::Duration;

use anyhow::{Context, bail};
use clap::Subcommand;
use reqwest::{Method, RequestBuilder};
use serde_json::{Value, json};
use sessionweft_client_protocol::{ApiEnvelope, EventBatch};

#[derive(Debug, Subcommand)]
pub enum ClientCommand {
    Protocol,
    Events {
        #[arg(long, default_value_t = 0)]
        after: u64,
        #[arg(long, default_value_t = 100)]
        limit: u32,
        #[arg(long)]
        follow: bool,
    },
    View {
        session_id: String,
        #[arg(long)]
        agent_id: Option<String>,
        #[arg(long)]
        workflow_id: Option<String>,
        #[arg(long)]
        workspace_id: Option<String>,
    },
    PtyStart {
        session_id: String,
        program: String,
        #[arg(long, default_value = ".")]
        cwd: String,
        #[arg(long, default_value_t = 24)]
        rows: u16,
        #[arg(long, default_value_t = 80)]
        cols: u16,
        #[arg(long = "arg")]
        args: Vec<String>,
    },
    PtyInput {
        pty_id: String,
        data: String,
    },
    PtyResize {
        pty_id: String,
        rows: u16,
        cols: u16,
    },
    PtyOutput {
        pty_id: String,
        #[arg(long, default_value_t = 0)]
        after: u64,
        #[arg(long, default_value_t = 1000)]
        wait_ms: u64,
        #[arg(long)]
        follow: bool,
    },
    PtyCancel {
        pty_id: String,
    },
}

pub async fn execute(
    client: &reqwest::Client,
    endpoint: &str,
    token: Option<&str>,
    command: &ClientCommand,
) -> anyhow::Result<()> {
    match command {
        ClientCommand::Protocol => {
            let value = request(client, endpoint, token, Method::GET, "/v1/client/protocol", None)
                .await?;
            print_pretty(&value)
        }
        ClientCommand::Events {
            after,
            limit,
            follow,
        } => events(client, endpoint, token, *after, *limit, *follow).await,
        ClientCommand::View {
            session_id,
            agent_id,
            workflow_id,
            workspace_id,
        } => {
            let mut query = Vec::new();
            if let Some(value) = agent_id {
                query.push(("agent_id", value.as_str()));
            }
            if let Some(value) = workflow_id {
                query.push(("workflow_id", value.as_str()));
            }
            if let Some(value) = workspace_id {
                query.push(("workspace_id", value.as_str()));
            }
            let suffix = serde_urlencoded::to_string(query).context("failed to encode query")?;
            let path = if suffix.is_empty() {
                format!("/v1/sessions/{session_id}/client-view")
            } else {
                format!("/v1/sessions/{session_id}/client-view?{suffix}")
            };
            let value = request(client, endpoint, token, Method::GET, &path, None).await?;
            print_pretty(&value)
        }
        ClientCommand::PtyStart {
            session_id,
            program,
            cwd,
            rows,
            cols,
            args,
        } => {
            let value = request(
                client,
                endpoint,
                token,
                Method::POST,
                "/v1/pty",
                Some(json!({
                    "session_id": session_id,
                    "program": program,
                    "args": args,
                    "cwd": cwd,
                    "environment": {},
                    "rows": rows,
                    "cols": cols,
                    "max_output_bytes": sessionweft_client_protocol::DEFAULT_PTY_OUTPUT_LIMIT,
                })),
            )
            .await?;
            print_pretty(&value)
        }
        ClientCommand::PtyInput { pty_id, data } => {
            request(
                client,
                endpoint,
                token,
                Method::POST,
                &format!("/v1/pty/{pty_id}/input"),
                Some(json!({"data": data})),
            )
            .await?;
            Ok(())
        }
        ClientCommand::PtyResize { pty_id, rows, cols } => {
            request(
                client,
                endpoint,
                token,
                Method::POST,
                &format!("/v1/pty/{pty_id}/resize"),
                Some(json!({"rows": rows, "cols": cols})),
            )
            .await?;
            Ok(())
        }
        ClientCommand::PtyOutput {
            pty_id,
            after,
            wait_ms,
            follow,
        } => pty_output(client, endpoint, token, pty_id, *after, *wait_ms, *follow).await,
        ClientCommand::PtyCancel { pty_id } => {
            let value = request(
                client,
                endpoint,
                token,
                Method::POST,
                &format!("/v1/pty/{pty_id}/cancel"),
                None,
            )
            .await?;
            print_pretty(&value)
        }
    }
}

async fn events(
    client: &reqwest::Client,
    endpoint: &str,
    token: Option<&str>,
    mut after: u64,
    limit: u32,
    follow: bool,
) -> anyhow::Result<()> {
    loop {
        let value = request(
            client,
            endpoint,
            token,
            Method::GET,
            &format!("/v1/events?after={after}&limit={limit}"),
            None,
        )
        .await?;
        let envelope: ApiEnvelope<EventBatch> =
            serde_json::from_value(value.clone()).context("invalid Runtime event envelope")?;
        if follow {
            for record in &envelope.data.events {
                println!("{}", serde_json::to_string(record)?);
            }
        } else {
            return print_pretty(&value);
        }
        after = envelope.data.next.0;
        if envelope.data.events.is_empty() {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

async fn pty_output(
    client: &reqwest::Client,
    endpoint: &str,
    token: Option<&str>,
    pty_id: &str,
    mut after: u64,
    wait_ms: u64,
    follow: bool,
) -> anyhow::Result<()> {
    loop {
        let value = request(
            client,
            endpoint,
            token,
            Method::GET,
            &format!("/v1/pty/{pty_id}/output?after={after}&wait_ms={wait_ms}"),
            None,
        )
        .await?;
        let next = value
            .pointer("/data/next")
            .and_then(Value::as_u64)
            .unwrap_or(after);
        if follow {
            if let Some(chunks) = value.pointer("/data/chunks").and_then(Value::as_array) {
                for chunk in chunks {
                    if let Some(data) = chunk.get("data").and_then(Value::as_str) {
                        print!("{data}");
                    }
                }
            }
        } else {
            return print_pretty(&value);
        }
        after = next;
        let status = value
            .pointer("/data/status")
            .and_then(Value::as_str)
            .unwrap_or("running");
        if status != "running" {
            return Ok(());
        }
    }
}

async fn request(
    client: &reqwest::Client,
    endpoint: &str,
    token: Option<&str>,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> anyhow::Result<Value> {
    let mut builder = client.request(method, format!("{endpoint}{path}"));
    builder = authorize(builder, token);
    if let Some(body) = body {
        builder = builder.json(&body);
    }
    let response = builder.send().await.context("failed to reach Runtime")?;
    let status = response.status();
    let bytes = response.bytes().await.context("failed to read Runtime response")?;
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    if !status.is_success() {
        print_pretty(&value)?;
        bail!("Runtime request failed with HTTP {status}");
    }
    Ok(value)
}

fn authorize(builder: RequestBuilder, token: Option<&str>) -> RequestBuilder {
    token.map_or(builder.try_clone().unwrap_or(builder), |token| {
        builder.bearer_auth(token)
    })
}

fn print_pretty(value: &Value) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
