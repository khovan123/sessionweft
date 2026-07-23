use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

#[derive(Debug, Parser)]
#[command(
    name = "sessionweft-agentctl",
    version,
    about = "Control persistent Codex, Claude, Gemini and Antigravity processes on durable SessionWeft sessions"
)]
struct Cli {
    #[arg(
        long,
        env = "SESSIONWEFT_AGENT_ENDPOINT",
        default_value = "http://127.0.0.1:7449"
    )]
    endpoint: String,

    #[arg(long, env = "SESSIONWEFT_AGENT_API_TOKEN", hide_env_values = true)]
    token: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Health,
    Sessions {
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    Create {
        title: String,
    },
    Get {
        session_id: String,
    },
    Status {
        session_id: String,
    },
    Start {
        session_id: String,
        agent: String,
        #[arg(long, default_value = ".")]
        cwd: String,
        #[arg(long = "arg")]
        args: Vec<String>,
        #[arg(long, default_value_t = 30)]
        rows: u16,
        #[arg(long, default_value_t = 120)]
        cols: u16,
    },
    Switch {
        session_id: String,
        agent: String,
    },
    Resume {
        session_id: String,
        agent: String,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long = "arg")]
        args: Vec<String>,
    },
    Send {
        session_id: String,
        message: String,
        #[arg(long)]
        agent: Option<String>,
    },
    History {
        session_id: String,
        #[arg(long, default_value_t = 0)]
        after: u64,
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    Context {
        session_id: String,
    },
    Stop {
        session_id: String,
        agent: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let client = reqwest::Client::new();
    let endpoint = cli.endpoint.trim_end_matches('/');
    let (method, path, body) = match cli.command {
        Command::Health => (Method::GET, "/health/ready".into(), None),
        Command::Sessions { limit } => (Method::GET, format!("/v1/sessions?limit={limit}"), None),
        Command::Create { title } => (
            Method::POST,
            "/v1/sessions".into(),
            Some(json!({"title": title})),
        ),
        Command::Get { session_id } => (Method::GET, format!("/v1/sessions/{session_id}"), None),
        Command::Status { session_id } => (
            Method::GET,
            format!("/v1/sessions/{session_id}/standalone-agents"),
            None,
        ),
        Command::Start {
            session_id,
            agent,
            cwd,
            args,
            rows,
            cols,
        } => (
            Method::POST,
            format!("/v1/sessions/{session_id}/standalone-agents/{agent}/start"),
            Some(json!({
                "cwd": cwd,
                "args": args,
                "rows": rows,
                "cols": cols,
            })),
        ),
        Command::Switch { session_id, agent } => (
            Method::POST,
            format!("/v1/sessions/{session_id}/standalone-agents/{agent}/switch"),
            None,
        ),
        Command::Resume {
            session_id,
            agent,
            cwd,
            args,
        } => (
            Method::POST,
            format!("/v1/sessions/{session_id}/standalone-agents/{agent}/resume"),
            Some(json!({"cwd": cwd, "args": args})),
        ),
        Command::Send {
            session_id,
            message,
            agent,
        } => {
            let path = match agent {
                Some(agent) => format!("/v1/sessions/{session_id}/standalone-agents/{agent}/send"),
                None => format!("/v1/sessions/{session_id}/standalone-agents/send"),
            };
            (Method::POST, path, Some(json!({"message": message})))
        }
        Command::History {
            session_id,
            after,
            limit,
        } => (
            Method::GET,
            format!(
                "/v1/sessions/{session_id}/standalone-agents/history?after={after}&limit={limit}"
            ),
            None,
        ),
        Command::Context { session_id } => (
            Method::GET,
            format!("/v1/sessions/{session_id}/standalone-agents/context"),
            None,
        ),
        Command::Stop { session_id, agent } => (
            Method::POST,
            format!("/v1/sessions/{session_id}/standalone-agents/{agent}/stop"),
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
    let response = request
        .send()
        .await
        .with_context(|| format!("failed to reach standalone agent daemon at {endpoint}"))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .context("read agent daemon response")?;
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    if !status.is_success() {
        print_json(&value)?;
        bail!("standalone agent request failed with HTTP {status}");
    }
    if status != StatusCode::NO_CONTENT {
        print_json(&value)?;
    }
    Ok(())
}

fn print_json(value: &Value) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
