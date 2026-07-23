use std::{
    env,
    io::{self, IsTerminal},
};

use anyhow::{Context, bail};
use reqwest::{Client, Method};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy)]
pub(super) enum CliFlavor {
    OneShot,
    Persistent,
}

#[derive(Debug, Default)]
pub(super) struct DisplayPlan {
    pub(super) handled: bool,
    refresh_session: Option<String>,
    operation: Option<String>,
    agent: Option<String>,
    config: Option<EndpointConfig>,
}

pub(super) fn before(flavor: CliFlavor, args: &[String]) -> DisplayPlan {
    if args.len() <= 1
        || args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--help" | "-h" | "--version" | "-V"))
    {
        return DisplayPlan::default();
    }

    match try_before(flavor, args) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("warning: could not display Session details: {error:#}");
            DisplayPlan::default()
        }
    }
}

pub(super) fn after(_flavor: CliFlavor, plan: &DisplayPlan, succeeded: bool) {
    if !succeeded {
        return;
    }
    let Some(session_id) = plan.refresh_session.as_deref() else {
        return;
    };

    let Some(config) = plan.config.as_ref() else {
        return;
    };
    match request_json(
        config,
        Method::GET,
        &format!("/v1/sessions/{session_id}"),
        None,
    ) {
        Ok(session) => eprint!(
            "{}",
            render_session_card(
                "SESSION UPDATED",
                &session,
                plan.operation.as_deref(),
                plan.agent.as_deref(),
            )
        ),
        Err(error) => eprintln!("warning: could not refresh Session {session_id}: {error:#}"),
    }
}

fn try_before(flavor: CliFlavor, args: &[String]) -> anyhow::Result<DisplayPlan> {
    let parsed = ParsedArgs::new(args)?;
    let config = endpoint_config(flavor, args);
    let stdout_is_terminal = io::stdout().is_terminal();

    if parsed.command == "sessions" && stdout_is_terminal {
        let limit = parsed
            .option("--limit")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(100);
        let sessions = request_json(
            &config,
            Method::GET,
            &format!("/v1/sessions?limit={limit}"),
            None,
        )?;
        print_sessions(&sessions)?;
        return Ok(DisplayPlan {
            handled: true,
            ..DisplayPlan::default()
        });
    }

    let create_command = match flavor {
        CliFlavor::OneShot => "new",
        CliFlavor::Persistent => "create",
    };
    if parsed.command == create_command && stdout_is_terminal {
        let title = parsed
            .positionals
            .first()
            .context("Session title is required")?;
        let session = request_json(
            &config,
            Method::POST,
            "/v1/sessions",
            Some(json!({"title": title})),
        )?;
        print!(
            "{}",
            render_session_card("SESSION CREATED", &session, Some(create_command), None)
        );
        return Ok(DisplayPlan {
            handled: true,
            ..DisplayPlan::default()
        });
    }

    let (session_id, agent, title, refresh_after) = match flavor {
        CliFlavor::OneShot => one_shot_target(&parsed),
        CliFlavor::Persistent => persistent_target(&parsed),
    };

    if let Some(session_id) = session_id.as_deref() {
        let session = request_json(
            &config,
            Method::GET,
            &format!("/v1/sessions/{session_id}"),
            None,
        )?;
        eprint!(
            "{}",
            render_session_card(
                "SESSION",
                &session,
                Some(&parsed.command),
                agent.as_deref(),
            )
        );
    } else if matches!(parsed.command.as_str(), "run" | "resume") {
        eprint!(
            "{}",
            render_new_session_preview(
                title.as_deref().unwrap_or("Shared agent session"),
                agent.as_deref(),
            )
        );
    }

    Ok(DisplayPlan {
        handled: false,
        refresh_session: if refresh_after { session_id } else { None },
        operation: Some(parsed.command),
        agent,
        config: Some(config),
    })
}

fn one_shot_target(parsed: &ParsedArgs) -> (Option<String>, Option<String>, Option<String>, bool) {
    match parsed.command.as_str() {
        "run" | "resume" => (
            parsed.option("--session").map(str::to_owned),
            parsed.positionals.first().cloned(),
            parsed
                .option("--title")
                .map(str::to_owned)
                .or_else(|| Some("Shared agent session".to_owned())),
            true,
        ),
        "history" | "context" => (
            parsed.positionals.first().cloned(),
            None,
            None,
            false,
        ),
        _ => (None, None, None, false),
    }
}

fn persistent_target(
    parsed: &ParsedArgs,
) -> (Option<String>, Option<String>, Option<String>, bool) {
    let session_commands = [
        "get", "status", "start", "switch", "resume", "send", "history", "context", "stop",
    ];
    if !session_commands.contains(&parsed.command.as_str()) {
        return (None, None, None, false);
    }

    let agent = match parsed.command.as_str() {
        "start" | "switch" | "resume" | "stop" => parsed.positionals.get(1).cloned(),
        "send" => parsed.option("--agent").map(str::to_owned),
        _ => None,
    };
    let refresh_after = matches!(
        parsed.command.as_str(),
        "start" | "switch" | "resume" | "send" | "stop"
    );
    (
        parsed.positionals.first().cloned(),
        agent,
        None,
        refresh_after,
    )
}

#[derive(Debug, Clone)]
struct EndpointConfig {
    endpoint: String,
    token: Option<String>,
}

fn endpoint_config(flavor: CliFlavor, args: &[String]) -> EndpointConfig {
    let parsed_endpoint = option_value(args, "--endpoint").map(str::to_owned);
    let parsed_token = option_value(args, "--token").map(str::to_owned);
    let (endpoint_env, token_env, default_endpoint) = match flavor {
        CliFlavor::OneShot => (
            "SESSIONWEFT_ENDPOINT",
            "SESSIONWEFT_API_TOKEN",
            "http://127.0.0.1:7447",
        ),
        CliFlavor::Persistent => (
            "SESSIONWEFT_AGENT_ENDPOINT",
            "SESSIONWEFT_AGENT_API_TOKEN",
            "http://127.0.0.1:7449",
        ),
    };

    EndpointConfig {
        endpoint: parsed_endpoint
            .or_else(|| env::var(endpoint_env).ok())
            .unwrap_or_else(|| default_endpoint.to_owned())
            .trim_end_matches('/')
            .to_owned(),
        token: parsed_token.or_else(|| env::var(token_env).ok()),
    }
}

fn request_json(
    config: &EndpointConfig,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> anyhow::Result<Value> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build Session display runtime")?;
    runtime.block_on(async {
        let client = Client::new();
        let mut request = client.request(method, format!("{}{}", config.endpoint, path));
        if let Some(token) = config.token.as_deref() {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.context("reach SessionWeft Runtime")?;
        let status = response.status();
        let bytes = response.bytes().await.context("read Runtime response")?;
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                Value::String(String::from_utf8_lossy(&bytes).into_owned())
            })
        };
        if !status.is_success() {
            bail!("Runtime returned HTTP {status}: {value}");
        }
        Ok(value)
    })
}

fn print_sessions(value: &Value) -> anyhow::Result<()> {
    let sessions = value
        .as_array()
        .context("Runtime Session list response is not an array")?;
    println!("SESSIONS ({})", sessions.len());
    if sessions.is_empty() {
        println!("  No durable Sessions found.");
        return Ok(());
    }
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
    println!("\nUse a Session ID with `--session <ID>` or the persistent agent command.");
    Ok(())
}

fn render_session_card(
    heading: &str,
    session: &Value,
    operation: Option<&str>,
    agent: Option<&str>,
) -> String {
    let id = field_string(session, "id")
        .or_else(|| field_string(session, "session_id"))
        .unwrap_or("-");
    let title = field_string(session, "title").unwrap_or("Untitled Session");
    let version = session
        .get("version")
        .or_else(|| session.get("session_version"))
        .and_then(Value::as_u64)
        .map_or_else(|| "-".to_owned(), |value| value.to_string());
    let message_count = session
        .get("messages")
        .and_then(Value::as_array)
        .map(Vec::len)
        .map_or_else(|| "-".to_owned(), |value| value.to_string());
    let updated = field_string(session, "updated_at").unwrap_or("-");

    let mut output = format!(
        "\n{heading}\n  id:       {id}\n  title:    {title}\n  version:  {version}\n  messages: {message_count}\n  updated:  {updated}\n"
    );
    if let Some(operation) = operation {
        output.push_str(&format!("  command:  {operation}\n"));
    }
    if let Some(agent) = agent {
        output.push_str(&format!("  agent:    {agent}\n"));
    }
    output.push('\n');
    output
}

fn render_new_session_preview(title: &str, agent: Option<&str>) -> String {
    let mut output = format!(
        "\nSESSION\n  mode:     create new\n  id:       assigned by Runtime\n  title:    {title}\n"
    );
    if let Some(agent) = agent {
        output.push_str(&format!("  agent:    {agent}\n"));
    }
    output.push('\n');
    output
}

fn field_string<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

#[derive(Debug)]
struct ParsedArgs {
    command: String,
    positionals: Vec<String>,
    args: Vec<String>,
}

impl ParsedArgs {
    fn new(args: &[String]) -> anyhow::Result<Self> {
        let command_index = command_index(args).context("agent command is required")?;
        let command = args[command_index].clone();
        let positionals = positional_values(args, command_index + 1);
        Ok(Self {
            command,
            positionals,
            args: args.to_vec(),
        })
    }

    fn option(&self, name: &str) -> Option<&str> {
        option_value(&self.args, name)
    }
}

fn command_index(args: &[String]) -> Option<usize> {
    let mut index = 1;
    while index < args.len() {
        let argument = args[index].as_str();
        if matches!(argument, "--endpoint" | "--token") {
            index += 2;
        } else if argument.starts_with("--endpoint=") || argument.starts_with("--token=") {
            index += 1;
        } else if argument.starts_with('-') {
            index += 1;
        } else {
            return Some(index);
        }
    }
    None
}

fn positional_values(args: &[String], start: usize) -> Vec<String> {
    let value_options = [
        "--endpoint",
        "--token",
        "--session",
        "--title",
        "--cwd",
        "--context-messages",
        "--messages",
        "--limit",
        "--arg",
        "--rows",
        "--cols",
        "--agent",
    ];
    let mut values = Vec::new();
    let mut index = start;
    while index < args.len() {
        let argument = args[index].as_str();
        if value_options.contains(&argument) {
            index += 2;
        } else if argument.starts_with("--") {
            index += 1;
        } else if argument.starts_with('-') {
            index += 1;
        } else {
            values.push(args[index].clone());
            index += 1;
        }
    }
    values
}

fn option_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let prefix = format!("{name}=");
    let mut index = 1;
    while index < args.len() {
        let argument = args[index].as_str();
        if argument == name {
            return args.get(index + 1).map(String::as_str);
        }
        if let Some(value) = argument.strip_prefix(&prefix) {
            return Some(value);
        }
        index += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn parses_one_shot_resume_session_and_agent() {
        let parsed = ParsedArgs::new(&args(&[
            "sessionweft-agent",
            "resume",
            "fcc-claude",
            "continue",
            "--session",
            "session-1",
        ]))
        .expect("parse");
        let (session, agent, _, refresh) = one_shot_target(&parsed);
        assert_eq!(session.as_deref(), Some("session-1"));
        assert_eq!(agent.as_deref(), Some("fcc-claude"));
        assert!(refresh);
    }

    #[test]
    fn parses_persistent_session_and_agent() {
        let parsed = ParsedArgs::new(&args(&[
            "sessionweft-agentctl",
            "switch",
            "session-2",
            "claude",
        ]))
        .expect("parse");
        let (session, agent, _, refresh) = persistent_target(&parsed);
        assert_eq!(session.as_deref(), Some("session-2"));
        assert_eq!(agent.as_deref(), Some("claude"));
        assert!(refresh);
    }

    #[test]
    fn session_card_contains_identity_and_version() {
        let session = json!({
            "id": "session-3",
            "title": "Shared work",
            "version": 7,
            "messages": [{"role": "user", "content": "hello"}],
            "updated_at": "2026-07-24T00:00:00Z"
        });
        let card = render_session_card("SESSION", &session, Some("resume"), Some("codex"));
        assert!(card.contains("session-3"));
        assert!(card.contains("Shared work"));
        assert!(card.contains("version:  7"));
        assert!(card.contains("messages: 1"));
        assert!(card.contains("agent:    codex"));
    }
}
