use std::{
    env,
    ffi::OsString,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::{Context, bail};
use clap::Parser;
use crossterm::{
    event::{self, Event as TerminalEvent, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use reqwest::Client;
use serde_json::{Value, json};

#[derive(Debug, Parser)]
#[command(
    name = "sessionweft-agent-wrapper",
    version,
    about = "Select or create a shared Session, then open the native agent CLI"
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

    #[arg(long)]
    agent: Option<String>,

    #[arg(last = true)]
    passthrough: Vec<OsString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentKind {
    Codex,
    Claude,
    Gemini,
    FccClaude,
    FccCodex,
    Antigravity,
}

impl AgentKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::FccClaude => "fcc-claude",
            Self::FccCodex => "fcc-codex",
            Self::Antigravity => "antigravity",
        }
    }

    const fn program(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::FccClaude => "fcc-claude",
            Self::FccCodex => "fcc-codex",
            Self::Antigravity => "antigravity-ide",
        }
    }

    fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "codex" | "sw-codex" => Ok(Self::Codex),
            "claude" | "claude-code" | "sw-claude" => Ok(Self::Claude),
            "gemini" | "gemini-cli" | "sw-gemini" => Ok(Self::Gemini),
            "fcc-claude" | "sw-fcc-claude" => Ok(Self::FccClaude),
            "fcc-codex" | "sw-fcc-codex" => Ok(Self::FccCodex),
            "antigravity" | "antigravity-ide" | "sw-antigravity" => Ok(Self::Antigravity),
            other => bail!("unsupported wrapped agent '{other}'"),
        }
    }
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
    let value = response.json::<Value>().await.context("decode Runtime response")?;
    if !status.is_success() {
        bail!("SessionWeft Runtime returned HTTP {status}: {value}");
    }
    Ok(value)
}

#[derive(Debug, Clone)]
struct SessionRow {
    id: String,
    title: String,
    version: u64,
    messages: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerMode {
    Home,
    Resume,
    Search,
    Create,
}

struct PickerApp {
    agent: AgentKind,
    mode: PickerMode,
    home_index: usize,
    session_index: usize,
    sessions: Vec<SessionRow>,
    filtered: Vec<usize>,
    search: String,
    title: String,
    status: String,
}

impl PickerApp {
    fn new(agent: AgentKind, sessions: Vec<SessionRow>) -> Self {
        let filtered = (0..sessions.len()).collect();
        Self {
            agent,
            mode: PickerMode::Home,
            home_index: 0,
            session_index: 0,
            sessions,
            filtered,
            search: String::new(),
            title: String::new(),
            status: String::new(),
        }
    }

    fn refresh_filter(&mut self) {
        let needle = self.search.to_ascii_lowercase();
        self.filtered = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                needle.is_empty()
                    || row.title.to_ascii_lowercase().contains(&needle)
                    || row.id.to_ascii_lowercase().contains(&needle)
            })
            .map(|(index, _)| index)
            .collect();
        self.session_index = self.session_index.min(self.filtered.len().saturating_sub(1));
    }

    fn selected_session(&self) -> Option<&SessionRow> {
        let index = *self.filtered.get(self.session_index)?;
        self.sessions.get(index)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cwd = std::fs::canonicalize(&cli.cwd)
        .with_context(|| format!("resolve wrapper working directory {}", cli.cwd.display()))?;
    let agent = resolve_agent(cli.agent.as_deref())?;
    let runtime = RuntimeClient::new(cli.endpoint, cli.token);

    let session = if let Some(session_id) = cli.session.as_deref() {
        runtime.get(&format!("/v1/sessions/{session_id}")).await?
    } else if let Some(title) = cli.new.as_deref() {
        runtime.create_session(title).await?
    } else {
        select_session_tui(&runtime, agent).await?
    };

    print_session(&session, agent);
    materialize_wrapper_context(&cwd, &session, agent)?;
    launch_native(agent, &cwd, &cli.passthrough)
}

async fn select_session_tui(runtime: &RuntimeClient, agent: AgentKind) -> anyhow::Result<Value> {
    let sessions_value = runtime.get("/v1/sessions?limit=100").await?;
    let sessions = parse_sessions(&sessions_value)?;
    let mut app = PickerApp::new(agent, sessions);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_picker(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    match result? {
        PickerResult::Create(title) => runtime.create_session(&title).await,
        PickerResult::Resume(id) => runtime.get(&format!("/v1/sessions/{id}")).await,
        PickerResult::Quit => bail!("session selection cancelled"),
    }
}

#[derive(Debug)]
enum PickerResult {
    Create(String),
    Resume(String),
    Quit,
}

fn run_picker(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut PickerApp,
) -> anyhow::Result<PickerResult> {
    loop {
        terminal.draw(|frame| render_picker(frame, app))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let TerminalEvent::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match app.mode {
            PickerMode::Home => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    app.home_index = app.home_index.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    app.home_index = (app.home_index + 1).min(2);
                }
                KeyCode::Enter => match app.home_index {
                    0 => app.mode = PickerMode::Create,
                    1 => app.mode = PickerMode::Resume,
                    _ => return Ok(PickerResult::Quit),
                },
                KeyCode::Esc | KeyCode::Char('q') => return Ok(PickerResult::Quit),
                _ => {}
            },
            PickerMode::Resume => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    app.session_index = app.session_index.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    app.session_index =
                        (app.session_index + 1).min(app.filtered.len().saturating_sub(1));
                }
                KeyCode::Char('/') => app.mode = PickerMode::Search,
                KeyCode::Enter => {
                    if let Some(session) = app.selected_session() {
                        return Ok(PickerResult::Resume(session.id.clone()));
                    }
                    app.status = "No Session selected".into();
                }
                KeyCode::Esc => app.mode = PickerMode::Home,
                _ => {}
            },
            PickerMode::Search => match key.code {
                KeyCode::Enter => app.mode = PickerMode::Resume,
                KeyCode::Esc => {
                    app.search.clear();
                    app.refresh_filter();
                    app.mode = PickerMode::Resume;
                }
                KeyCode::Backspace => {
                    app.search.pop();
                    app.refresh_filter();
                }
                KeyCode::Char(value) => {
                    app.search.push(value);
                    app.refresh_filter();
                }
                _ => {}
            },
            PickerMode::Create => match key.code {
                KeyCode::Enter => {
                    let title = app.title.trim();
                    let title = if title.is_empty() {
                        "Shared coding session"
                    } else {
                        title
                    };
                    return Ok(PickerResult::Create(title.to_owned()));
                }
                KeyCode::Esc => {
                    app.title.clear();
                    app.mode = PickerMode::Home;
                }
                KeyCode::Backspace => {
                    app.title.pop();
                }
                KeyCode::Char(value) => app.title.push(value),
                _ => {}
            },
        }
    }
}

fn render_picker(frame: &mut Frame<'_>, app: &PickerApp) {
    let area = centered_rect(72, 78, frame.area());
    let shell = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(vec![
            Span::styled(" SessionWeft ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("{} ", app.agent.label())),
        ]));
    frame.render_widget(shell, area);

    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(match app.mode {
            PickerMode::Home => "Choose how to start this agent",
            PickerMode::Resume | PickerMode::Search => "Resume a shared Session",
            PickerMode::Create => "Create a new shared Session",
        })
        .alignment(Alignment::Center),
        chunks[0],
    );

    match app.mode {
        PickerMode::Home => render_home(frame, app, chunks[1]),
        PickerMode::Resume | PickerMode::Search => render_sessions(frame, app, chunks[1]),
        PickerMode::Create => render_create(frame, app, chunks[1]),
    }

    let help = match app.mode {
        PickerMode::Home => "↑/↓ Navigate   Enter Select   Esc Quit",
        PickerMode::Resume => "↑/↓ Navigate   Enter Resume   / Search   Esc Back",
        PickerMode::Search => "Type to filter   Enter Done   Esc Clear",
        PickerMode::Create => "Type a title   Enter Create   Esc Back",
    };
    let footer = if app.status.is_empty() {
        help.to_owned()
    } else {
        format!("{}   •   {}", app.status, help)
    };
    frame.render_widget(
        Paragraph::new(footer).alignment(Alignment::Center).wrap(Wrap { trim: true }),
        chunks[2],
    );
}

fn render_home(frame: &mut Frame<'_>, app: &PickerApp, area: ratatui::layout::Rect) {
    let items = ["Create new Session", "Resume existing Session", "Quit"]
        .into_iter()
        .map(ListItem::new)
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    state.select(Some(app.home_index));
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Start "))
        .highlight_symbol("› ")
        .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_sessions(frame: &mut Frame<'_>, app: &PickerApp, area: ratatui::layout::Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(4)])
        .split(area);
    let search = if app.mode == PickerMode::Search {
        format!("{}▏", app.search)
    } else if app.search.is_empty() {
        "Press / to search".to_owned()
    } else {
        app.search.clone()
    };
    frame.render_widget(
        Paragraph::new(search).block(Block::default().borders(Borders::ALL).title(" Search ")),
        chunks[0],
    );

    let items = app
        .filtered
        .iter()
        .filter_map(|index| app.sessions.get(*index))
        .map(|row| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<28}", truncate(&row.title, 28)),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("  v{:<4} {:>4} msg  {}", row.version, row.messages, short_id(&row.id))),
            ]))
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    if !items.is_empty() {
        state.select(Some(app.session_index));
    }
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(format!(
            " Sessions ({}) ",
            app.filtered.len()
        )))
        .highlight_symbol("› ")
        .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED));
    frame.render_stateful_widget(list, chunks[1], &mut state);
}

fn render_create(frame: &mut Frame<'_>, app: &PickerApp, area: ratatui::layout::Rect) {
    let input = if app.title.is_empty() {
        "Shared coding session▏".to_owned()
    } else {
        format!("{}▏", app.title)
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);
    frame.render_widget(
        Paragraph::new(input).block(Block::default().borders(Borders::ALL).title(" Session title ")),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new("The selected Session will be materialized and opened in the native agent CLI.")
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        chunks[1],
    );
}

fn centered_rect(percent_x: u16, percent_y: u16, area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn parse_sessions(value: &Value) -> anyhow::Result<Vec<SessionRow>> {
    let sessions = value.as_array().context("Session list is not an array")?;
    Ok(sessions
        .iter()
        .filter_map(|session| {
            Some(SessionRow {
                id: session.get("id")?.as_str()?.to_owned(),
                title: session
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Untitled Session")
                    .to_owned(),
                version: session.get("version").and_then(Value::as_u64).unwrap_or(0),
                messages: session
                    .get("messages")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len),
            })
        })
        .collect())
}

fn resolve_agent(explicit: Option<&str>) -> anyhow::Result<AgentKind> {
    if let Some(value) = explicit {
        return AgentKind::parse(value);
    }
    let executable = env::args_os()
        .next()
        .and_then(|path| PathBuf::from(path).file_name().map(OsString::from))
        .and_then(|name| name.into_string().ok())
        .context("resolve wrapper executable name")?;
    AgentKind::parse(&executable)
}

fn launch_native(agent: AgentKind, cwd: &Path, args: &[OsString]) -> anyhow::Result<()> {
    let status = Command::new(agent.program())
        .args(args)
        .current_dir(cwd)
        .env("SESSIONWEFT_WRAPPED_AGENT", agent.label())
        .env("SESSIONWEFT_CONTEXT_FILE", cwd.join(".sessionweft/active-context.md"))
        .env("SESSIONWEFT_SESSION_FILE", cwd.join(".sessionweft/active-session"))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("launch native wrapped agent {}", agent.program()))?;
    if !status.success() {
        bail!("native wrapped agent {} exited with {status}", agent.program());
    }
    Ok(())
}

fn materialize_wrapper_context(cwd: &Path, session: &Value, agent: AgentKind) -> anyhow::Result<()> {
    let directory = cwd.join(".sessionweft");
    std::fs::create_dir_all(&directory)?;
    let id = required_string(session, "id")?;
    let title = session
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Untitled Session");
    let version = session.get("version").and_then(Value::as_u64).unwrap_or(0);
    let mut content = format!(
        "# SessionWeft active context\n\n- Session ID: `{id}`\n- Title: {title}\n- Version: {version}\n- Agent: {}\n\n## Shared history\n",
        agent.label()
    );
    if let Some(messages) = session.get("messages").and_then(Value::as_array) {
        for message in messages.iter().rev().take(200).rev() {
            let role = message.get("role").and_then(Value::as_str).unwrap_or("unknown");
            let body = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            content.push_str(&format!("\n### {role}\n\n{body}\n"));
        }
    }
    std::fs::write(directory.join("active-context.md"), content)?;
    std::fs::write(directory.join("active-session"), id)?;
    Ok(())
}

fn print_session(session: &Value, agent: AgentKind) {
    println!("SESSION");
    println!("  id:      {}", session.get("id").and_then(Value::as_str).unwrap_or("-"));
    println!(
        "  title:   {}",
        session
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Untitled Session")
    );
    println!(
        "  version: {}",
        session.get("version").and_then(Value::as_u64).unwrap_or(0)
    );
    println!("  agent:   {}", agent.label());
    println!("Opening native {} CLI...\n", agent.label());
}

fn required_string(value: &Value, field: &str) -> anyhow::Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("Runtime response is missing '{field}'"))
}

fn short_id(value: &str) -> &str {
    value.get(..8).unwrap_or(value)
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_owned();
    }
    value.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_aliases_map_to_native_agents() {
        assert_eq!(AgentKind::parse("sw-codex").unwrap(), AgentKind::Codex);
        assert_eq!(
            AgentKind::parse("sw-fcc-claude").unwrap(),
            AgentKind::FccClaude
        );
    }

    #[test]
    fn search_filter_matches_title_and_id() {
        let mut app = PickerApp::new(
            AgentKind::Codex,
            vec![SessionRow {
                id: "abc-123".into(),
                title: "Runtime work".into(),
                version: 1,
                messages: 2,
            }],
        );
        app.search = "runtime".into();
        app.refresh_filter();
        assert_eq!(app.filtered, vec![0]);
        app.search = "missing".into();
        app.refresh_filter();
        assert!(app.filtered.is_empty());
    }
}
