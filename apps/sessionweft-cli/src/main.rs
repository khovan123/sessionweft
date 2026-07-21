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

fn print_json(value: &Value) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).context("failed to serialize output")?
    );
    Ok(())
}
