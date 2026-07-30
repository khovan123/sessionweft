use std::{
    collections::BTreeMap,
    fs,
    io::{self, Stdout},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use clap::Parser;
use crossterm::{
    event::{self, Event as TerminalEvent, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use reqwest::StatusCode;
use serde_json::{Value, json};
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
const CONFIG_FIELDS: [&str; 4] = ["workspace", "owner", "skills", "plugins"];

#[derive(Debug, Parser)]
#[command(
    name = "sessionweft-workflow",
    version,
    about = "Configure and run SessionWeft workflow agents from a TUI"
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

    #[arg(long, env = "SESSIONWEFT_SESSION_ID")]
    session_id: Option<Uuid>,

    #[arg(long, env = "SESSIONWEFT_WORKFLOW_ID")]
    workflow_id: Option<Uuid>,

    #[arg(long)]
    node_id: Option<String>,

    #[arg(
        long,
        env = "SESSIONWEFT_WORKFLOW_AGENT_CONFIG",
        default_value = ".sessionweft/workflow-agents.json"
    )]
    config: PathBuf,

    #[arg(long)]
    workspace_id: Option<String>,

    #[arg(long)]
    owner_id: Option<String>,

    #[arg(long)]
    skill: Vec<String>,

    #[arg(long)]
    plugin: Vec<String>,
}

#[derive(Debug, Clone)]
struct AgentPreset {
    workspace_id: String,
    owner_id: String,
    skills: Vec<String>,
    plugins: Vec<String>,
}

impl Default for AgentPreset {
    fn default() -> Self {
        Self {
            workspace_id: "default".into(),
            owner_id: "operator".into(),
            skills: Vec::new(),
            plugins: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct WorkflowAgentConfig {
    session_id: Option<Uuid>,
    workflow_id: Option<Uuid>,
    selected_agent: usize,
    agents: BTreeMap<String, AgentPreset>,
}

impl Default for WorkflowAgentConfig {
    fn default() -> Self {
        Self {
            session_id: None,
            workflow_id: None,
            selected_agent: 0,
            agents: AGENTS
                .iter()
                .map(|agent| ((*agent).to_owned(), AgentPreset::default()))
                .collect(),
        }
    }
}

impl WorkflowAgentConfig {
    fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let value: Value = serde_json::from_slice(&fs::read(path)?)
            .with_context(|| format!("decode workflow agent config {}", path.display()))?;
        let mut config = Self::default();
        config.session_id = value
            .get("session_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok());
        config.workflow_id = value
            .get("workflow_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok());
        config.selected_agent = value
            .get("selected_agent")
            .and_then(Value::as_str)
            .and_then(|name| AGENTS.iter().position(|agent| *agent == name))
            .unwrap_or(0);

        if let Some(agents) = value.get("agents").and_then(Value::as_object) {
            for agent in AGENTS {
                let Some(raw) = agents.get(agent) else {
                    continue;
                };
                let preset = config.agents.entry(agent.to_owned()).or_default();
                preset.workspace_id = raw
                    .get("workspace_id")
                    .and_then(Value::as_str)
                    .unwrap_or("default")
                    .to_owned();
                preset.owner_id = raw
                    .get("owner_id")
                    .and_then(Value::as_str)
                    .unwrap_or("operator")
                    .to_owned();
                preset.skills = string_array(raw.get("skills"));
                preset.plugins = string_array(raw.get("plugins"));
            }
        }
        Ok(config)
    }

    fn save(&self, path: &Path) -> anyhow::Result<()> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let agents = self
            .agents
            .iter()
            .map(|(name, preset)| {
                (
                    name.clone(),
                    json!({
                        "workspace_id": preset.workspace_id,
                        "owner_id": preset.owner_id,
                        "skills": preset.skills,
                        "plugins": preset.plugins,
                    }),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let value = json!({
            "schema_version": 1,
            "session_id": self.session_id.map(|value| value.to_string()),
            "workflow_id": self.workflow_id.map(|value| value.to_string()),
            "selected_agent": AGENTS[self.selected_agent.min(AGENTS.len() - 1)],
            "agents": agents,
        });
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(&value)?)?;
        fs::rename(&temporary, path)?;
        Ok(())
    }

    fn validate(&self) -> anyhow::Result<()> {
        for (agent, preset) in &self.agents {
            if preset.workspace_id.trim().is_empty() {
                bail!("{agent} workspace ID cannot be empty");
            }
            if preset.owner_id.trim().is_empty() {
                bail!("{agent} owner ID cannot be empty");
            }
        }
        Ok(())
    }
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Agents,
    Config,
    Task,
    Terminal,
}

struct App {
    client: reqwest::Client,
    endpoint: String,
    token: Option<String>,
    config_path: PathBuf,
    config: WorkflowAgentConfig,
    session_id: Uuid,
    workflow_id: Uuid,
    preferred_node_id: Option<String>,
    selected_node: usize,
    selected_config_field: usize,
    editing_config: bool,
    view: Option<ClientResourceView>,
    events: Vec<String>,
    cursor: EventCursor,
    status: String,
    last_refresh: Instant,
    mode: Mode,
    task: String,
    execution: Option<AgentExecutionView>,
    terminal_cursor: u64,
    terminal_output: String,
}

impl App {
    fn new(cli: Cli) -> anyhow::Result<Self> {
        let mut config = WorkflowAgentConfig::load(&cli.config)?;
        if let Some(value) = cli.session_id {
            config.session_id = Some(value);
        }
        if let Some(value) = cli.workflow_id {
            config.workflow_id = Some(value);
        }
        let selected_agent = config.selected_agent.min(AGENTS.len() - 1);
        config.selected_agent = selected_agent;
        let preset = config
            .agents
            .entry(AGENTS[selected_agent].to_owned())
            .or_default();
        if let Some(value) = cli.workspace_id {
            preset.workspace_id = value;
        }
        if let Some(value) = cli.owner_id {
            preset.owner_id = value;
        }
        if !cli.skill.is_empty() {
            preset.skills = cli.skill;
        }
        if !cli.plugin.is_empty() {
            preset.plugins = cli.plugin;
        }
        let session_id = config.session_id.context(
            "session ID is required once via --session-id or SESSIONWEFT_SESSION_ID",
        )?;
        let workflow_id = config.workflow_id.context(
            "workflow ID is required once via --workflow-id or SESSIONWEFT_WORKFLOW_ID",
        )?;
        config.save(&cli.config)?;

        Ok(Self {
            client: reqwest::Client::new(),
            endpoint: cli.endpoint.trim_end_matches('/').to_owned(),
            token: cli.token,
            config_path: cli.config,
            config,
            session_id,
            workflow_id,
            preferred_node_id: cli.node_id,
            selected_node: 0,
            selected_config_field: 0,
            editing_config: false,
            view: None,
            events: Vec::new(),
            cursor: EventCursor::default(),
            status: "connecting".into(),
            last_refresh: Instant::now() - Duration::from_secs(5),
            mode: Mode::Agents,
            task: String::new(),
            execution: None,
            terminal_cursor: 0,
            terminal_output: String::new(),
        })
    }

    fn selected_agent_name(&self) -> &'static str {
        AGENTS[self.config.selected_agent]
    }

    fn selected_preset(&self) -> &AgentPreset {
        self.config
            .agents
            .get(self.selected_agent_name())
            .expect("all supported agents have a preset")
    }

    fn selected_preset_mut(&mut self) -> &mut AgentPreset {
        let name = self.selected_agent_name().to_owned();
        self.config.agents.entry(name).or_default()
    }

    fn move_agent(&mut self, delta: isize) {
        let current = self.config.selected_agent as isize;
        self.config.selected_agent = (current + delta)
            .clamp(0, AGENTS.len().saturating_sub(1) as isize)
            as usize;
    }

    fn workflow_nodes(&self) -> Vec<(String, String)> {
        let Some(workflow) = self.view.as_ref().and_then(|view| view.workflow.as_ref()) else {
            return Vec::new();
        };
        workflow
            .get("definition")
            .and_then(|value| value.get("nodes"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|definition| {
                let id = definition.get("id")?.as_str()?.to_owned();
                let status = workflow
                    .get("nodes")
                    .and_then(|nodes| nodes.get(&id))
                    .and_then(|node| node.get("status"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned();
                Some((id, status))
            })
            .collect()
    }

    fn selected_node_id(&self) -> Option<String> {
        self.workflow_nodes()
            .get(self.selected_node)
            .map(|(id, _)| id.clone())
    }

    fn move_node(&mut self, delta: isize) {
        let len = self.workflow_nodes().len();
        if len == 0 {
            self.selected_node = 0;
            return;
        }
        self.selected_node = (self.selected_node as isize + delta)
            .clamp(0, len.saturating_sub(1) as isize) as usize;
    }

    fn sync_preferred_node(&mut self) {
        let nodes = self.workflow_nodes();
        if let Some(preferred) = self.preferred_node_id.take()
            && let Some(index) = nodes.iter().position(|(id, _)| id == &preferred)
        {
            self.selected_node = index;
        }
        self.selected_node = self.selected_node.min(nodes.len().saturating_sub(1));
    }

    async fn refresh(&mut self) {
        match self.fetch_view().await {
            Ok(view) => {
                self.view = Some(view);
                self.sync_preferred_node();
                self.status = "connected".into();
            }
            Err(error) => self.status = format!("offline: {error}"),
        }
        if let Ok(batch) = self.fetch_events().await {
            self.cursor = batch.next;
            self.events.extend(batch.events.into_iter().map(|record| {
                format!("{}  {}", record.cursor.0, record.envelope.event_type)
            }));
            if self.events.len() > 100 {
                self.events.drain(..self.events.len() - 100);
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
        let preset = self.selected_preset();
        let response = self
            .authorized(self.client.get(format!(
                "{}/v1/sessions/{}/client-view",
                self.endpoint, self.session_id
            )))
            .query(&[
                ("workflow_id", self.workflow_id.to_string()),
                ("workspace_id", preset.workspace_id.clone()),
            ])
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
            .await?;
        ensure_success(response.status())?;
        Ok(response.json::<ApiEnvelope<EventBatch>>().await?.data)
    }

    async fn start_execution(&mut self, rows: u16, cols: u16) {
        if self.task.trim().is_empty() {
            self.status = "task cannot be empty".into();
            return;
        }
        let Some(node_id) = self.selected_node_id() else {
            self.status = "workflow contains no selectable nodes".into();
            return;
        };
        let expected_version = self
            .view
            .as_ref()
            .and_then(|view| view.workflow.as_ref())
            .and_then(|value| value.get("version"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let preset = self.selected_preset().clone();
        let request = StartAgentExecutionRequest {
            expected_version,
            agent: self.selected_agent_name().into(),
            workspace_id: preset.workspace_id,
            owner_id: preset.owner_id,
            task: self.task.clone(),
            skills: preset.skills,
            plugins: preset.plugins,
            terminal: TerminalSize { cols, rows },
        };
        let response = self
            .authorized(self.client.post(format!(
                "{}/v1/sessions/{}/workflows/{}/nodes/{}/executions",
                self.endpoint, self.session_id, self.workflow_id, node_id
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
                        self.status = format!(
                            "started {} on node {node_id}",
                            self.selected_agent_name()
                        );
                    }
                    Err(error) => self.status = format!("invalid execution response: {error}"),
                }
            }
            Ok(response) => {
                self.status = format!("execution start failed: HTTP {}", response.status())
            }
            Err(error) => self.status = format!("execution start failed: {error}"),
        }
    }

    async fn fetch_execution(&self, execution_id: Uuid) -> anyhow::Result<AgentExecutionView> {
        let response = self
            .authorized(
                self.client
                    .get(format!("{}/v1/executions/{execution_id}", self.endpoint)),
            )
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
            self.status = "no active execution".into();
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
                self.mode = Mode::Agents;
            }
            Ok(response) => self.status = format!("stop failed: HTTP {}", response.status()),
            Err(error) => self.status = format!("stop failed: {error}"),
        }
    }

    fn config_value(&self, field: usize) -> String {
        let preset = self.selected_preset();
        match field {
            0 => preset.workspace_id.clone(),
            1 => preset.owner_id.clone(),
            2 => preset.skills.join(", "),
            3 => preset.plugins.join(", "),
            _ => String::new(),
        }
    }

    fn set_config_value(&mut self, field: usize, value: String) {
        let preset = self.selected_preset_mut();
        match field {
            0 => preset.workspace_id = value,
            1 => preset.owner_id = value,
            2 => preset.skills = parse_csv(&value),
            3 => preset.plugins = parse_csv(&value),
            _ => {}
        }
    }

    fn append_config_character(&mut self, character: char) {
        let field = self.selected_config_field;
        let mut value = self.config_value(field);
        value.push(character);
        self.set_config_value(field, value);
    }

    fn backspace_config(&mut self) {
        let field = self.selected_config_field;
        let mut value = self.config_value(field);
        value.pop();
        self.set_config_value(field, value);
    }

    fn save_config(&mut self) {
        match self.config.save(&self.config_path) {
            Ok(()) => self.status = format!("saved {}", self.config_path.display()),
            Err(error) => self.status = format!("config save failed: {error}"),
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

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut app = App::new(Cli::parse())?;
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
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        let TerminalEvent::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match app.mode {
            Mode::Agents => match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Char('r') => app.refresh().await,
                KeyCode::Char('c') | KeyCode::Char('2') => app.mode = Mode::Config,
                KeyCode::Char('t') | KeyCode::Enter => app.mode = Mode::Task,
                KeyCode::Char('o') if app.execution.is_some() => app.mode = Mode::Terminal,
                KeyCode::Char('x') => app.stop_execution().await,
                KeyCode::Up | KeyCode::Char('k') => app.move_agent(-1),
                KeyCode::Down | KeyCode::Char('j') => app.move_agent(1),
                KeyCode::Left | KeyCode::Char('h') => app.move_node(-1),
                KeyCode::Right | KeyCode::Char('l') => app.move_node(1),
                _ => {}
            },
            Mode::Config => {
                if app.editing_config {
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter => app.editing_config = false,
                        KeyCode::Backspace => app.backspace_config(),
                        KeyCode::Tab => {
                            app.editing_config = false;
                            app.selected_config_field =
                                (app.selected_config_field + 1) % CONFIG_FIELDS.len();
                        }
                        KeyCode::Char(character) => app.append_config_character(character),
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Esc | KeyCode::Char('1') => app.mode = Mode::Agents,
                        KeyCode::Char('s') => app.save_config(),
                        KeyCode::Up | KeyCode::Char('k') => {
                            app.selected_config_field =
                                app.selected_config_field.saturating_sub(1);
                        }
                        KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                            app.selected_config_field =
                                (app.selected_config_field + 1) % CONFIG_FIELDS.len();
                        }
                        KeyCode::Left | KeyCode::Char('h') => app.move_agent(-1),
                        KeyCode::Right | KeyCode::Char('l') => app.move_agent(1),
                        KeyCode::Enter => app.editing_config = true,
                        _ => {}
                    }
                }
            }
            Mode::Task => match key.code {
                KeyCode::Esc => app.mode = Mode::Agents,
                KeyCode::Enter => {
                    let area = terminal.size()?;
                    app.start_execution(area.height.saturating_sub(4), area.width)
                        .await;
                }
                KeyCode::Backspace => {
                    app.task.pop();
                }
                KeyCode::Tab => app.move_agent(1),
                KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => app.move_node(-1),
                KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => app.move_node(1),
                KeyCode::Char(character) => app.task.push(character),
                _ => {}
            },
            Mode::Terminal => match key.code {
                KeyCode::Esc => app.mode = Mode::Agents,
                KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
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

fn render(frame: &mut Frame<'_>, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(14),
            Constraint::Length(5),
        ])
        .split(frame.area());

    render_header(frame, app, rows[0]);
    match app.mode {
        Mode::Agents => render_agents(frame, app, rows[1]),
        Mode::Config => render_config(frame, app, rows[1]),
        Mode::Task => render_task(frame, app, rows[1]),
        Mode::Terminal => render_terminal(frame, app, rows[1]),
    }
    render_footer(frame, app, rows[2]);
}

fn render_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let execution = app.execution.as_ref().map_or_else(
        || "idle".to_owned(),
        |value| format!("{} {:?}", short_id(&value.execution_id.to_string()), value.state),
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            tab(" 1 Workflow Agents ", app.mode == Mode::Agents),
            Span::raw(" "),
            tab(" 2 Agent Config ", app.mode == Mode::Config),
            Span::raw(format!(
                "   session {}   workflow {}   {execution}",
                short_id(&app.session_id.to_string()),
                short_id(&app.workflow_id.to_string())
            )),
        ]))
        .block(Block::default().borders(Borders::ALL).title("SessionWeft")),
        area,
    );
}

fn tab(label: &'static str, selected: bool) -> Span<'static> {
    if selected {
        Span::styled(
            label,
            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED),
        )
    } else {
        Span::styled(label, Style::default().add_modifier(Modifier::BOLD))
    }
}

fn render_agents(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(24),
            Constraint::Percentage(38),
            Constraint::Percentage(38),
        ])
        .split(area);

    let agents = AGENTS
        .iter()
        .map(|agent| ListItem::new(*agent))
        .collect::<Vec<_>>();
    let mut agent_state = ListState::default();
    agent_state.select(Some(app.config.selected_agent));
    frame.render_stateful_widget(
        List::new(agents)
            .block(Block::default().borders(Borders::ALL).title("Agents"))
            .highlight_symbol("› ")
            .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)),
        columns[0],
        &mut agent_state,
    );

    let nodes = app.workflow_nodes();
    let node_items = nodes
        .iter()
        .map(|(id, status)| ListItem::new(format!("{:<28} {status}", truncate(id, 28))))
        .collect::<Vec<_>>();
    let mut node_state = ListState::default();
    if !nodes.is_empty() {
        node_state.select(Some(app.selected_node));
    }
    frame.render_stateful_widget(
        List::new(node_items)
            .block(Block::default().borders(Borders::ALL).title("Workflow nodes"))
            .highlight_symbol("› ")
            .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)),
        columns[1],
        &mut node_state,
    );

    let preset = app.selected_preset();
    let details = format!(
        "Agent\n  {}\n\nSelected node\n  {}\n\nWorkspace\n  {}\n\nOwner\n  {}\n\nSkills\n  {}\n\nPlugins\n  {}",
        app.selected_agent_name(),
        app.selected_node_id().unwrap_or_else(|| "-".into()),
        preset.workspace_id,
        preset.owner_id,
        empty_as_dash(&preset.skills.join(", ")),
        empty_as_dash(&preset.plugins.join(", ")),
    );
    frame.render_widget(
        Paragraph::new(details)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Resolved agent configuration"),
            ),
        columns[2],
    );
}

fn render_config(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(28), Constraint::Percentage(72)])
        .split(area);

    let agents = AGENTS
        .iter()
        .map(|agent| ListItem::new(*agent))
        .collect::<Vec<_>>();
    let mut agent_state = ListState::default();
    agent_state.select(Some(app.config.selected_agent));
    frame.render_stateful_widget(
        List::new(agents)
            .block(Block::default().borders(Borders::ALL).title("Agent presets"))
            .highlight_symbol("› ")
            .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)),
        columns[0],
        &mut agent_state,
    );

    let fields = CONFIG_FIELDS
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let cursor = if app.editing_config && index == app.selected_config_field {
                "█"
            } else {
                ""
            };
            ListItem::new(format!(
                "{:<12} {}{}",
                label,
                app.config_value(index),
                cursor
            ))
        })
        .collect::<Vec<_>>();
    let mut field_state = ListState::default();
    field_state.select(Some(app.selected_config_field));
    frame.render_stateful_widget(
        List::new(fields)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!("{} configuration", app.selected_agent_name())),
            )
            .highlight_symbol("› ")
            .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)),
        columns[1],
        &mut field_state,
    );
}

fn render_task(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let preset = app.selected_preset();
    frame.render_widget(
        Paragraph::new(format!(
            "Agent: {}\nNode: {}\nWorkspace: {}\nOwner: {}\nSkills: {}\nPlugins: {}\n\nTask:\n{}█",
            app.selected_agent_name(),
            app.selected_node_id().unwrap_or_else(|| "-".into()),
            preset.workspace_id,
            preset.owner_id,
            empty_as_dash(&preset.skills.join(", ")),
            empty_as_dash(&preset.plugins.join(", ")),
            app.task,
        ))
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Workflow task · Enter start · Tab agent · Ctrl-←/→ node · Esc back"),
        ),
        area,
    );
}

fn render_terminal(frame: &mut Frame<'_>, app: &App, area: Rect) {
    frame.render_widget(
        Paragraph::new(app.terminal_output.clone())
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Runtime-owned agent terminal · Esc agents · Ctrl-X stop"),
            ),
        area,
    );
}

fn render_footer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let help = match app.mode {
        Mode::Agents => "↑/↓ agent   ←/→ node   Enter task   c config   o terminal   x stop   q quit",
        Mode::Config if app.editing_config => {
            "Type value   Enter/Esc finish field   Tab next field"
        }
        Mode::Config => "↑/↓ field   ←/→ agent   Enter edit   s save   Esc agents   q quit",
        Mode::Task => "Type task   Enter start   Tab agent   Ctrl-←/→ node   Esc agents",
        Mode::Terminal => "Native terminal input   Esc agents   Ctrl-X stop",
    };
    let latest_event = app.events.last().map_or("-", String::as_str);
    frame.render_widget(
        Paragraph::new(format!(
            "{}\n{}\nlatest event: {}",
            help, app.status, latest_event
        ))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title("Status")),
        area,
    );
}

fn empty_as_dash(value: &str) -> &str {
    if value.trim().is_empty() { "-" } else { value }
}

fn short_id(value: &str) -> &str {
    value.get(..8).unwrap_or(value)
}

fn truncate(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        return value.to_owned();
    }
    value
        .chars()
        .take(maximum.saturating_sub(1))
        .collect::<String>()
        + "…"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_values_are_trimmed_and_empty_items_removed() {
        assert_eq!(
            parse_csv("rust, github, , filesystem"),
            vec!["rust", "github", "filesystem"]
        );
    }

    #[test]
    fn config_round_trip_preserves_agent_presets() {
        let root = std::env::temp_dir().join(format!("sessionweft-config-{}", Uuid::new_v4()));
        let path = root.join("workflow-agents.json");
        let mut config = WorkflowAgentConfig::default();
        config.session_id = Some(Uuid::new_v4());
        config.workflow_id = Some(Uuid::new_v4());
        config.selected_agent = 1;
        config.agents.get_mut("claude").unwrap().skills = vec!["rust".into()];
        config.save(&path).unwrap();

        let loaded = WorkflowAgentConfig::load(&path).unwrap();
        assert_eq!(loaded.session_id, config.session_id);
        assert_eq!(loaded.workflow_id, config.workflow_id);
        assert_eq!(loaded.selected_agent, 1);
        assert_eq!(loaded.agents["claude"].skills, vec!["rust"]);
        let _ = fs::remove_dir_all(root);
    }
}
