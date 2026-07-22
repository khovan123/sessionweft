use std::{
    io::{self, Stdout},
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use clap::Parser;
use crossterm::{
    event::{self, Event as TerminalEvent, KeyCode},
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
use sessionweft_client_protocol::{ApiEnvelope, ClientResourceView, EventBatch, EventCursor};
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(name = "sessionweft-tui", version, about = "SessionWeft Runtime TUI")]
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
}

struct App {
    client: reqwest::Client,
    endpoint: String,
    token: Option<String>,
    session_id: Uuid,
    agent_id: Option<Uuid>,
    workflow_id: Option<Uuid>,
    workspace_id: Option<String>,
    cursor: EventCursor,
    view: Option<ClientResourceView>,
    events: Vec<String>,
    selected_approval: usize,
    status: String,
    last_refresh: Instant,
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
            workspace_id: cli.workspace_id,
            cursor: EventCursor::default(),
            view: None,
            events: Vec::new(),
            selected_approval: 0,
            status: "connecting".into(),
            last_refresh: Instant::now() - Duration::from_secs(5),
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
        match self.fetch_events().await {
            Ok(batch) => {
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
            Err(error) => self.status = format!("event stream unavailable: {error}"),
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
        if let Some(workspace_id) = &self.workspace_id {
            query.push(("workspace_id", workspace_id.clone()));
        }
        let request = self
            .authorized(self.client.get(format!(
                "{}/v1/sessions/{}/client-view",
                self.endpoint, self.session_id
            )))
            .query(&query);
        let response = request.send().await.context("failed to reach Runtime")?;
        ensure_success(response.status())?;
        let envelope = response
            .json::<ApiEnvelope<ClientResourceView>>()
            .await
            .context("invalid client-view response")?;
        Ok(envelope.data)
    }

    async fn fetch_events(&self) -> anyhow::Result<EventBatch> {
        let response = self
            .authorized(self.client.get(format!("{}/v1/events", self.endpoint)))
            .query(&[("after", self.cursor.0), ("limit", 100)])
            .send()
            .await
            .context("failed to reach event endpoint")?;
        ensure_success(response.status())?;
        let envelope = response
            .json::<ApiEnvelope<EventBatch>>()
            .await
            .context("invalid event response")?;
        Ok(envelope.data)
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
                self.refresh().await;
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
        if app.last_refresh.elapsed() >= Duration::from_secs(1) {
            app.refresh().await;
        }
        terminal.draw(|frame| render(frame, app))?;
        if event::poll(Duration::from_millis(100))?
            && let TerminalEvent::Key(key) = event::read()?
        {
            match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Char('r') => app.refresh().await,
                KeyCode::Char('a') => app.decide_approval(true).await,
                KeyCode::Char('d') => app.decide_approval(false).await,
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
            }
        }
    }
}

fn render(frame: &mut Frame<'_>, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Percentage(45),
            Constraint::Percentage(35),
            Constraint::Percentage(20),
        ])
        .split(frame.area());
    frame.render_widget(
        Paragraph::new(format!(
            "Session {} | cursor {} | {} | q quit · r refresh · a approve · d deny",
            app.session_id, app.cursor.0, app.status
        ))
        .block(Block::default().borders(Borders::ALL).title("SessionWeft")),
        rows[0],
    );

    let resource_columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);
    let session_text = app
        .view
        .as_ref()
        .map(|view| pretty(&view.session))
        .unwrap_or_else(|| "No session snapshot".into());
    frame.render_widget(
        Paragraph::new(session_text)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title("Session")),
        resource_columns[0],
    );
    let workflow_text = app
        .view
        .as_ref()
        .and_then(|view| view.workflow.as_ref())
        .map(pretty)
        .unwrap_or_else(|| "No workflow selected".into());
    frame.render_widget(
        Paragraph::new(workflow_text)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Workflow / Agent / Locks"),
            ),
        resource_columns[1],
    );

    let event_items = app
        .events
        .iter()
        .rev()
        .take(50)
        .map(|event| ListItem::new(event.clone()))
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
                    let marker = if index == app.selected_approval {
                        ">"
                    } else {
                        " "
                    };
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
