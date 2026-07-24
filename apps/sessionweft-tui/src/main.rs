use std::{
    io::{self, Stdout},
    time::{Duration, Instant},
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
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use reqwest::StatusCode;
use serde_json::Value;
use sessionweft_client_protocol::{
    AgentExecutionView, ApiEnvelope, ClientResourceView, EventBatch, EventCursor,
    StartAgentExecutionRequest, StartAgentExecutionResponse, StopAgentExecutionRequest,
    TerminalFrameBatch, TerminalInputRequest, TerminalSize,
};
use uuid::Uuid;

const AGENTS: [&str; 6] = [
    "codex",
    "claude",
    "gemini",
    "fcc-claude",
    "fcc-codex",
    "antigravity-ide",
];

#[derive(Debug, Parser)]
#[command(name = "sessionweft-tui", version, about = "SessionWeft Runtime execution TUI")]
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
    session_id: Uuid,
    #[arg(long)]
    agent_id: Option<Uuid>,
    #[arg(long)]
    workflow_id: Option<Uuid>,
    #[arg(long)]
    workspace_id: Option<String>,
    #[arg(long, default_value = "interactive")]
    node_id: String,
    #[arg(long, default_value = "tui")]
    owner_id: String,
    #[arg(long)]
    skill: Vec<String>,
    #[arg(long)]
    plugin: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Monitor,
    Task,
    Terminal,
}

struct App {
    client: reqwest::Client,
    endpoint: String,
    token: Option<String>,
    session_id: Uuid,
    agent_id: Option<Uuid>,
    workflow_id: Option<Uuid>,
    workspace_id: String,
    node_id: String,
    owner_id: String,
    skills: Vec<String>,
    plugins: Vec<String>,
    cursor: EventCursor,
    view: Option<ClientResourceView>,
    events: Vec<String>,
    selected_approval: usize,
    status: String,
    last_refresh: Instant,
    mode: Mode,
    task: String,
    selected_agent: usize,
    execution: Option<AgentExecutionView>,
    terminal_cursor: u64,
    terminal_output: String,
}

impl App {
    fn new(cli: Cli) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint: cli.endpoint.trim_end_matches('/').to_owned(),
            token: cli.token,
            session_id: cli.session_id,
            agent_id: cli.agent_id,
            workflow_id: cli.workflow_id,
            workspace_id: cli.workspace_id.unwrap_or_else(|| "default".into()),
            node_id: cli.node_id,
            owner_id: cli.owner_id,
            skills: cli.skill,
            plugins: cli.plugin,
            cursor: EventCursor::default(),
            view: None,
            events: Vec::new(),
            selected_approval: 0,
            status: "connecting".into(),
            last_refresh: Instant::now() - Duration::from_secs(5),
            mode: Mode::Monitor,
            task: String::new(),
            selected_agent: 0,
            execution: None,
            terminal_cursor: 0,
            terminal_output: String::new(),
        }
    }

    async fn refresh(&mut self) {
        match self.fetch_view().await {
            Ok(view) => {
                self.selected_approval = self
                    .selected_approval
                    .min(view.pending_approvals.len().saturating_sub(1));
                self.view = Some(view);
                self.status = "connected".into();
            }
            Err(error) => self.status = format!("offline: {error}"),
        }
        if let Ok(batch) = self.fetch_events().await {
            self.cursor = batch.next;
            self.events.extend(batch.events.into_iter().map(|record| {
                format!(
                    "{}  {}  {}",
                    record.cursor.0,
                    record.envelope.event_type,
                    record
                        .envelope
                        .session_id
                        .map_or_else(|| "global".into(), |value| value.to_string())
                )
            }));
            if self.events.len() > 200 {
                self.events.drain(..self.events.len() - 200);
            }
        }
        if let Some(execution) = self.execution.clone() {
            if let Ok(view) = self.fetch_execution(execution.execution_id).await {
                self.execution = Some(view);
            }
            if let Ok(batch) = self.fetch_terminal(execution.execution_id).await {
                self.terminal_cursor = batch.next_cursor;
                for frame in batch.frames {
                    self.terminal_output.push_str(&frame.data);
                }
                const MAX_TERMINAL_BYTES: usize = 2 * 1024 * 1024;
                if self.terminal_output.len() > MAX_TERMINAL_BYTES {
                    let keep_from = self.terminal_output.len() - MAX_TERMINAL_BYTES;
                    self.terminal_output.drain(..keep_from);
                }
            }
        }
        self.last_refresh = Instant::now();
    }

    async fn fetch_view(&self) -> anyhow::Result<ClientResourceView> {
        let mut query = vec![];
        if let Some(agent_id) = self.agent_id {
            query.push(("agent_id", agent_id.to_string()));
        }
        if let Some(workflow_id) = self.workflow_id {
            query.push(("workflow_id", workflow_id.to_string()));
        }
        query.push(("workspace_id", self.workspace_id.clone()));
        let response = self
            .authorized(self.client.get(format!(
                "{}/v1/sessions/{}/client-view",
                self.endpoint, self.session_id
            )))
            .query(&query)
            .send()
            .await
            .context("failed to reach Runtime")?;
        ensure_success(response.status())?;
        Ok(response
            .json::<ApiEnvelope<ClientResourceView>>()
            .await
            .context("invalid client-view response")?
            .data)
    }

    async fn fetch_events(&self) -> anyhow::Result<EventBatch> {
        let response = self
            .authorized(self.client.get(format!("{}/v1/events", self.endpoint)))
            .query(&[("after", self.cursor.0), ("limit", 100)])
            .send()
            .await
            .context("failed to reach event endpoint")?;
        ensure_success(response.status())?;
        Ok(response
            .json::<ApiEnvelope<EventBatch>>()
            .await
            .context("invalid event response")?
            .data)
    }

    async fn start_execution(&mut self, rows: u16, cols: u16) {
        let Some(workflow_id) = self.workflow_id else {
            self.status = "--workflow-id is required to start an agent workflow node".into();
            return;
        };
        if self.task.trim().is_empty() {
            self.status = "task cannot be empty".into();
            return;
        }
        let expected_version = self
            .view
            .as_ref()
            .and_then(|view| view.workflow.as_ref())
            .and_then(|value| value.get("version"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let request = StartAgentExecutionRequest {
            expected_version,
            agent: AGENTS[self.selected_agent].into(),
            workspace_id: self.workspace_id.clone(),
            owner_id: self.owner_id.clone(),
            task: self.task.clone(),
            skills: self.skills.clone(),
            plugins: self.plugins.clone(),
            terminal: TerminalSize { cols, rows },
        };
        let response = self
            .authorized(self.client.post(format!(
                "{}/v1/sessions/{}/workflows/{}/nodes/{}/executions",
                self.endpoint, self.session_id, workflow_id, self.node_id
            )))
            .json(&request)
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => {
                match response.json::<StartAgentExecutionResponse>().await {
                    Ok(started) => {
                        self.execution = Some(started.execution);
                        self.terminal_cursor = 0;
                        self.terminal_output.clear();
                        self.mode = Mode::Terminal;
                        self.status = "Runtime-owned agent execution started".into();
                    }
                    Err(error) => self.status = format!("invalid execution response: {error}"),
                }
            }
            Ok(response) => self.status = format!("execution start failed: HTTP {}", response.status()),
            Err(error) => self.status = format!("execution start failed: {error}"),
        }
    }

    async fn fetch_execution(&self, execution_id: Uuid) -> anyhow::Result<AgentExecutionView> {
        let response = self
            .authorized(self.client.get(format!(
                "{}/v1/executions/{execution_id}",
                self.endpoint
            )))
            .send()
            .await?;
        ensure_success(response.status())?;
        Ok(response.json().await?)
    }

    async fn fetch_terminal(&self, execution_id: Uuid) -> anyhow::Result<TerminalFrameBatch> {
        let response = self
            .authorized(self.client.get(format!(
                "{}/v1/executions/{execution_id}/terminal",
                self.endpoint
            )))
            .query(&[("after", self.terminal_cursor)])
            .send()
            .await?;
        ensure_success(response.status())?;
        Ok(response.json().await?)
    }

    async fn terminal_input(&mut self, data: String) {
        let Some(execution) = &self.execution else {
            return;
        };
        let response = self
            .authorized(self.client.post(format!(
                "{}/v1/executions/{}/terminal/input",
                self.endpoint, execution.execution_id
            )))
            .json(&TerminalInputRequest {
                fencing_token: execution.fencing_token,
                data,
            })
            .send()
            .await;
        if let Err(error) = response {
            self.status = format!("terminal input failed: {error}");
        }
    }

    async fn stop_execution(&mut self) {
        let Some(execution) = &self.execution else {
            return;
        };
        let response = self
            .authorized(self.client.post(format!(
                "{}/v1/executions/{}/stop",
                self.endpoint, execution.execution_id
            )))
            .json(&StopAgentExecutionRequest {
                fencing_token: execution.fencing_token,
            })
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => {
                self.status = "execution stopped".into();
                self.mode = Mode::Monitor;
            }
            Ok(response) => self.status = format!("stop failed: HTTP {}", response.status()),
            Err(error) => self.status = format!("stop failed: {error}"),
        }
    }

    async fn decide_approval(&mut self, approved: bool) {
        let Some(view) = &self.view else {
            return;
        };
        let Some(approval) = view.pending_approvals.get(self.selected_approval) else {
            self.status = "no pending approval selected".into();
            return;
        };
        let response = self
            .authorized(self.client.post(format!(
                "{}/v1/sessions/{}/workflows/{}/nodes/{}/approval",
                self.endpoint, self.session_id, approval.workflow_id, approval.node_id
            )))
            .json(&serde_json::json!({
                "expected_version": approval.expected_version,
                "approved": approved,
            }))
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => {
                self.status = if approved {
                    "approval granted".into()
                } else {
                    "approval rejected".into()
                };
            }
            Ok(response) => self.status = format!("approval failed: HTTP {}", response.status()),
            Err(error) => self.status = format!("approval failed: {error}"),
        }
    }

    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(token) = self.token.as_deref() {
            request.bearer_auth(token)
        } else {
            request
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut app = App::new(Cli::parse());
    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal, &mut app).await;
    restore_terminal(&mut terminal)?;
    result
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
) -> anyhow::Result<()> {
    loop {
        if app.last_refresh.elapsed() >= Duration::from_millis(500) {
            app.refresh().await;
        }
        terminal.draw(|frame| render(frame, app))?;
        if event::poll(Duration::from_millis(50))?
            && let TerminalEvent::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match app.mode {
                Mode::Monitor => match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Char('r') => app.refresh().await,
                    KeyCode::Char('a') => app.decide_approval(true).await,
                    KeyCode::Char('d') => app.decide_approval(false).await,
                    KeyCode::Char('t') => app.mode = Mode::Task,
                    KeyCode::Char('g') => {
                        app.selected_agent = (app.selected_agent + 1) % AGENTS.len();
                    }
                    KeyCode::Char('x') => app.stop_execution().await,
                    KeyCode::Char('o') if app.execution.is_some() => app.mode = Mode::Terminal,
                    KeyCode::Up => {
                        app.selected_approval = app.selected_approval.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        let maximum = app
                            .view
                            .as_ref()
                            .map_or(0, |view| view.pending_approvals.len().saturating_sub(1));
                        app.selected_approval = (app.selected_approval + 1).min(maximum);
                    }
                    _ => {}
                },
                Mode::Task => match key.code {
                    KeyCode::Esc => app.mode = Mode::Monitor,
                    KeyCode::Enter => {
                        let area = terminal.size()?;
                        app.start_execution(area.height.saturating_sub(4), area.width).await;
                    }
                    KeyCode::Backspace => {
                        app.task.pop();
                    }
                    KeyCode::Tab => {
                        app.selected_agent = (app.selected_agent + 1) % AGENTS.len();
                    }
                    KeyCode::Char(character) => app.task.push(character),
                    _ => {}
                },
                Mode::Terminal => match key.code {
                    KeyCode::Esc => app.mode = Mode::Monitor,
                    KeyCode::Char('x') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                        app.stop_execution().await;
                    }
                    KeyCode::Enter => app.terminal_input("\n".into()).await,
                    KeyCode::Backspace => app.terminal_input("\x08".into()).await,
                    KeyCode::Tab => app.terminal_input("\t".into()).await,
                    KeyCode::Char(character) => app.terminal_input(character.to_string()).await,
                    KeyCode::Up => app.terminal_input("\x1b[A".into()).await,
                    KeyCode::Down => app.terminal_input("\x1b[B".into()).await,
                    KeyCode::Right => app.terminal_input("\x1b[C".into()).await,
                    KeyCode::Left => app.terminal_input("\x1b[D".into()).await,
                    _ => {}
                },
            }
        }
    }
}

fn render(frame: &mut Frame<'_>, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Percentage(55),
            Constraint::Percentage(25),
            Constraint::Percentage(20),
        ])
        .split(frame.area());
    let execution = app.execution.as_ref().map_or_else(
        || "no execution".into(),
        |value| format!("{} {:?}", value.execution_id, value.state),
    );
    frame.render_widget(
        Paragraph::new(format!(
            "Session {} | agent {} | {} | mode {:?} | {}",
            app.session_id, AGENTS[app.selected_agent], execution, app.mode, app.status
        ))
        .block(Block::default().borders(Borders::ALL).title(
            "SessionWeft · t task · g agent · o terminal · x stop · a/d approval · q quit",
        )),
        rows[0],
    );

    match app.mode {
        Mode::Terminal => frame.render_widget(
            Paragraph::new(app.terminal_output.clone())
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title(
                    "Runtime-owned agent terminal · Esc monitor · Ctrl-X stop",
                )),
            rows[1],
        ),
        Mode::Task => frame.render_widget(
            Paragraph::new(format!(
                "Agent: {}\nSkills: {}\nPlugins: {}\n\nTask:\n{}█",
                AGENTS[app.selected_agent],
                app.skills.join(", "),
                app.plugins.join(", "),
                app.task
            ))
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(
                "Task editor · Enter start via Runtime · Tab agent · Esc cancel",
            )),
            rows[1],
        ),
        Mode::Monitor => {
            let columns = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(rows[1]);
            frame.render_widget(
                Paragraph::new(
                    app.view
                        .as_ref()
                        .map(|view| pretty(&view.session))
                        .unwrap_or_else(|| "No session snapshot".into()),
                )
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title("Session")),
                columns[0],
            );
            frame.render_widget(
                Paragraph::new(
                    app.view
                        .as_ref()
                        .and_then(|view| view.workflow.as_ref())
                        .map(pretty)
                        .unwrap_or_else(|| "No workflow selected".into()),
                )
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title(
                    "Workflow / Agent / Locks",
                )),
                columns[1],
            );
        }
    }

    let event_items = app
        .events
        .iter()
        .rev()
        .take(50)
        .map(|value| ListItem::new(value.clone()))
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(event_items).block(Block::default().borders(Borders::ALL).title("Events")),
        rows[2],
    );

    let approvals = app
        .view
        .as_ref()
        .map(|view| {
            view.pending_approvals
                .iter()
                .enumerate()
                .map(|(index, approval)| {
                    let marker = if index == app.selected_approval { ">" } else { " " };
                    ListItem::new(format!(
                        "{marker} {} / {} / version {}",
                        approval.workflow_id, approval.node_id, approval.expected_version
                    ))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    frame.render_widget(
        List::new(approvals).block(Block::default().borders(Borders::ALL).title("Approvals")),
        rows[3],
    );
}

fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout)).context("failed to initialize terminal")
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> anyhow::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn ensure_success(status: StatusCode) -> anyhow::Result<()> {
    if status.is_success() {
        Ok(())
    } else {
        bail!("Runtime returned HTTP {status}")
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}
