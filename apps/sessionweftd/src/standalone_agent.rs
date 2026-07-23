use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    env, fmt,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sessionweft_client_protocol::{
    DEFAULT_PTY_OUTPUT_LIMIT, PtyError, PtyStatus, PtySupervisor, StartPtyRequest,
};
use sessionweft_core::{DomainError, MessageRole, Session, SessionId};
use sessionweft_provider::ProviderRegistry;
use sessionweft_runtime::{RuntimeError, RuntimeService};
use sessionweft_storage::{SqliteSessionRepository, StorageError};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use uuid::Uuid;

pub type StandaloneRuntime = RuntimeService<SqliteSessionRepository>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StandaloneAgentKind {
    Codex,
    Claude,
    Gemini,
    AntigravityIde,
}

impl StandaloneAgentKind {
    pub const ALL: [Self; 4] = [
        Self::Codex,
        Self::Claude,
        Self::Gemini,
        Self::AntigravityIde,
    ];

    #[must_use]
    pub const fn program(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::AntigravityIde => "antigravity-ide",
        }
    }

    #[must_use]
    pub const fn direct_input_supported(self) -> bool {
        !matches!(self, Self::AntigravityIde)
    }

    #[must_use]
    pub const fn mode(self) -> &'static str {
        if self.direct_input_supported() {
            "interactive_cli"
        } else {
            "ide_context_bridge"
        }
    }

    fn default_args(self) -> Vec<String> {
        match self {
            Self::AntigravityIde => vec![".".into()],
            _ => Vec::new(),
        }
    }
}

impl fmt::Display for StandaloneAgentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::AntigravityIde => "antigravity_ide",
        };
        formatter.write_str(value)
    }
}

impl FromStr for StandaloneAgentKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "codex" => Ok(Self::Codex),
            "claude" | "claude_code" => Ok(Self::Claude),
            "gemini" | "gemini_cli" => Ok(Self::Gemini),
            "antigravity" | "antigravity_ide" | "anti" => Ok(Self::AntigravityIde),
            other => bail!(
                "unsupported standalone agent '{other}'; expected codex, claude, gemini or antigravity_ide"
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentAvailability {
    pub agent: StandaloneAgentKind,
    pub program: String,
    pub available: bool,
    pub mode: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct StandaloneAgentBindingView {
    pub session_id: SessionId,
    pub agent: StandaloneAgentKind,
    pub program: String,
    pub cwd: String,
    pub args: Vec<String>,
    pub pty_id: Option<Uuid>,
    pub status: String,
    pub active: bool,
    pub resume_required: bool,
    pub direct_input_supported: bool,
    pub output_cursor: u64,
    pub context_path: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StandaloneAgentSessionView {
    pub session_id: SessionId,
    pub active_agent: Option<StandaloneAgentKind>,
    pub agents: Vec<StandaloneAgentBindingView>,
    pub available_agents: Vec<AgentAvailability>,
    pub context_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StandaloneAgentHistoryEntry {
    pub sequence: u64,
    pub session_id: SessionId,
    pub agent: StandaloneAgentKind,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StandaloneAgentHistoryPage {
    pub after: u64,
    pub next: u64,
    pub entries: Vec<StandaloneAgentHistoryEntry>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct StandaloneAgentSendResult {
    pub session_id: SessionId,
    pub session_version: u64,
    pub agent: StandaloneAgentKind,
    pub direct_input_supported: bool,
    pub context_path: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StartStandaloneAgentRequest {
    pub cwd: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub rows: Option<u16>,
    pub cols: Option<u16>,
}

#[derive(Debug, Clone)]
struct BindingRow {
    session_id: SessionId,
    agent: StandaloneAgentKind,
    program: String,
    cwd: String,
    args: Vec<String>,
    pty_id: Option<Uuid>,
    output_cursor: u64,
    status: String,
    created_at: String,
    updated_at: String,
}

#[derive(Clone)]
pub struct StandaloneAgentManager {
    pool: SqlitePool,
    pty: Arc<PtySupervisor>,
    workspace_root: Arc<PathBuf>,
    available_programs: Arc<BTreeSet<String>>,
    capture_tasks: Arc<Mutex<HashMap<(SessionId, StandaloneAgentKind), Uuid>>>,
}

impl StandaloneAgentManager {
    pub async fn connect(
        database_url: &str,
        pty: Arc<PtySupervisor>,
        workspace_root: impl AsRef<Path>,
        available_programs: BTreeSet<String>,
    ) -> Result<Self> {
        let is_memory = database_url.contains(":memory:");
        let options = SqliteConnectOptions::from_str(database_url)
            .context("parse standalone agent database URL")?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(if is_memory { 1 } else { 5 })
            .connect_with(options)
            .await
            .context("connect standalone agent database")?;
        let workspace_root = std::fs::canonicalize(workspace_root)
            .context("resolve standalone agent workspace root")?;
        let manager = Self {
            pool,
            pty,
            workspace_root: Arc::new(workspace_root),
            available_programs: Arc::new(available_programs),
            capture_tasks: Arc::new(Mutex::new(HashMap::new())),
        };
        manager.migrate().await?;
        Ok(manager)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS standalone_agent_bindings (
                session_id TEXT NOT NULL,
                agent_kind TEXT NOT NULL,
                program TEXT NOT NULL,
                cwd TEXT NOT NULL,
                args_json TEXT NOT NULL,
                pty_id TEXT,
                output_cursor INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (session_id, agent_kind)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .context("create standalone agent bindings table")?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS standalone_agent_session_state (
                session_id TEXT PRIMARY KEY,
                active_agent_kind TEXT,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .context("create standalone agent session state table")?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS standalone_agent_history (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                agent_kind TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .context("create standalone agent history table")?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_standalone_agent_history_session_sequence
            ON standalone_agent_history (session_id, sequence)
            "#,
        )
        .execute(&self.pool)
        .await
        .context("create standalone agent history index")?;
        Ok(())
    }

    #[must_use]
    pub fn availability(&self) -> Vec<AgentAvailability> {
        StandaloneAgentKind::ALL
            .into_iter()
            .map(|agent| AgentAvailability {
                agent,
                program: agent.program().into(),
                available: self.available_programs.contains(agent.program()),
                mode: agent.mode(),
            })
            .collect()
    }

    pub async fn session_view(&self, session_id: SessionId) -> Result<StandaloneAgentSessionView> {
        let active = self.active_agent(session_id).await?;
        let rows = self.list_binding_rows(session_id).await?;
        let mut agents = Vec::with_capacity(rows.len());
        for row in rows {
            agents.push(self.binding_view(row, active).await?);
        }
        Ok(StandaloneAgentSessionView {
            session_id,
            active_agent: active,
            agents,
            available_agents: self.availability(),
            context_path: self.context_path(session_id).display().to_string(),
        })
    }

    pub async fn start_or_resume(
        &self,
        runtime: StandaloneRuntime,
        session: &Session,
        agent: StandaloneAgentKind,
        request: StartStandaloneAgentRequest,
    ) -> Result<StandaloneAgentBindingView> {
        if !self.available_programs.contains(agent.program()) {
            bail!(
                "standalone agent program '{}' was not found in PATH; install it or add it to SESSIONWEFT_STANDALONE_AGENT_PROGRAMS",
                agent.program()
            );
        }
        self.refresh_context(session).await?;
        let existing = self.binding_row(session.id, agent).await?;
        if let Some(row) = existing.as_ref()
            && let Some(pty_id) = row.pty_id
            && self
                .pty
                .descriptor(pty_id)
                .is_ok_and(|descriptor| descriptor.status == PtyStatus::Running)
        {
            self.set_active(session.id, agent).await?;
            return self.binding_view(row.clone(), Some(agent)).await;
        }

        let cwd = request
            .cwd
            .filter(|value| !value.trim().is_empty())
            .or_else(|| existing.as_ref().map(|row| row.cwd.clone()))
            .unwrap_or_else(|| ".".into());
        let args = if request.args.is_empty() {
            existing
                .as_ref()
                .map(|row| row.args.clone())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| agent.default_args())
        } else {
            request.args
        };
        let descriptor = self.pty.start(StartPtyRequest {
            session_id: session.id,
            program: agent.program().into(),
            args: args.clone(),
            cwd: cwd.clone(),
            environment: forwarded_environment(),
            rows: request.rows.unwrap_or(30),
            cols: request.cols.unwrap_or(120),
            max_output_bytes: DEFAULT_PTY_OUTPUT_LIMIT,
        })?;
        let now = Utc::now().to_rfc3339();
        let created_at = existing
            .as_ref()
            .map(|row| row.created_at.clone())
            .unwrap_or_else(|| now.clone());
        self.upsert_binding(
            session.id,
            agent,
            agent.program(),
            &cwd,
            &args,
            descriptor.pty_id,
            0,
            "running",
            &created_at,
            &now,
        )
        .await?;
        self.set_active(session.id, agent).await?;
        self.append_history(
            session.id,
            agent,
            "system",
            &format!(
                "{} {} for shared session {}",
                if existing.is_some() {
                    "resumed"
                } else {
                    "started"
                },
                agent,
                session.id
            ),
        )
        .await?;

        if agent.direct_input_supported() {
            let bootstrap = format!(
                "SessionWeft shared session {} is active. Read the durable context at {} and continue the same conversation across agents. Preserve prior decisions and history.\n",
                session.id,
                self.context_path(session.id).display()
            );
            self.pty.input(descriptor.pty_id, &bootstrap)?;
        }
        self.spawn_capture(runtime, session.id, agent, descriptor.pty_id, 0);
        let row = self
            .binding_row(session.id, agent)
            .await?
            .ok_or_else(|| anyhow!("standalone agent binding disappeared after start"))?;
        self.binding_view(row, Some(agent)).await
    }

    pub async fn switch(
        &self,
        runtime: StandaloneRuntime,
        session: &Session,
        agent: StandaloneAgentKind,
    ) -> Result<StandaloneAgentBindingView> {
        let request = self
            .binding_row(session.id, agent)
            .await?
            .map(|row| StartStandaloneAgentRequest {
                cwd: Some(row.cwd),
                args: row.args,
                rows: None,
                cols: None,
            })
            .unwrap_or_default();
        self.start_or_resume(runtime, session, agent, request).await
    }

    pub async fn send(
        &self,
        runtime: StandaloneRuntime,
        session_id: SessionId,
        requested_agent: Option<StandaloneAgentKind>,
        message: String,
    ) -> Result<StandaloneAgentSendResult> {
        if message.trim().is_empty() {
            bail!("standalone agent message cannot be empty");
        }
        if message.len() > 1_000_000 {
            bail!("standalone agent message exceeds 1 MiB");
        }
        let agent = match requested_agent {
            Some(agent) => agent,
            None => self.active_agent(session_id).await?.ok_or_else(|| {
                anyhow!("no active standalone agent is selected for this session")
            })?,
        };
        let binding = self.binding_row(session_id, agent).await?.ok_or_else(|| {
            anyhow!("standalone agent {agent} has not been started for this session")
        })?;

        let session = append_shared_message(
            &runtime,
            session_id,
            MessageRole::User,
            format!("[to {agent}]\n{message}"),
            &format!("standalone-agent:{agent}"),
        )
        .await?;
        self.append_history(session_id, agent, "user", &message)
            .await?;
        self.refresh_context(&session).await?;

        if agent.direct_input_supported() {
            let pty_id = binding.pty_id.ok_or_else(|| {
                anyhow!("standalone agent {agent} must be resumed before sending")
            })?;
            let descriptor = self.pty.descriptor(pty_id).map_err(|error| match error {
                PtyError::NotFound(_) | PtyError::NotRunning(_) => {
                    anyhow!("standalone agent {agent} must be resumed before sending")
                }
                other => anyhow!(other),
            })?;
            if descriptor.status != PtyStatus::Running {
                bail!("standalone agent {agent} is not running; resume it before sending");
            }
            self.pty.input(pty_id, &format!("{message}\n"))?;
        } else {
            self.write_active_prompt(session_id, agent, &message)
                .await?;
            self.append_history(
                session_id,
                agent,
                "system",
                "message staged in active-prompt.md for the Antigravity IDE context bridge",
            )
            .await?;
        }

        Ok(StandaloneAgentSendResult {
            session_id,
            session_version: session.version,
            agent,
            direct_input_supported: agent.direct_input_supported(),
            context_path: self.context_path(session_id).display().to_string(),
            message: if agent.direct_input_supported() {
                "message sent to running agent".into()
            } else {
                "message persisted to shared context; Antigravity IDE is launch/context bridge mode"
                    .into()
            },
        })
    }

    pub async fn stop(
        &self,
        session_id: SessionId,
        agent: StandaloneAgentKind,
    ) -> Result<StandaloneAgentBindingView> {
        let row = self.binding_row(session_id, agent).await?.ok_or_else(|| {
            anyhow!("standalone agent {agent} has not been started for this session")
        })?;
        if let Some(pty_id) = row.pty_id {
            match self.pty.cancel(pty_id) {
                Ok(_) | Err(PtyError::NotFound(_)) | Err(PtyError::NotRunning(_)) => {}
                Err(error) => return Err(error.into()),
            }
        }
        self.update_binding_progress(session_id, agent, row.output_cursor, "stopped")
            .await?;
        self.append_history(session_id, agent, "system", "agent stopped")
            .await?;
        let active = self.active_agent(session_id).await?;
        let row = self
            .binding_row(session_id, agent)
            .await?
            .ok_or_else(|| anyhow!("standalone agent binding disappeared after stop"))?;
        self.binding_view(row, active).await
    }

    pub async fn history(
        &self,
        session_id: SessionId,
        after: u64,
        limit: u32,
    ) -> Result<StandaloneAgentHistoryPage> {
        let limit = limit.clamp(1, 1_000);
        let rows = sqlx::query(
            r#"
            SELECT sequence, agent_kind, role, content, created_at
            FROM standalone_agent_history
            WHERE session_id = ? AND sequence > ?
            ORDER BY sequence ASC
            LIMIT ?
            "#,
        )
        .bind(session_id.to_string())
        .bind(to_i64(after)?)
        .bind(i64::from(limit) + 1)
        .fetch_all(&self.pool)
        .await
        .context("read standalone agent history")?;
        let has_more = rows.len() > limit as usize;
        let entries = rows
            .into_iter()
            .take(limit as usize)
            .map(|row| {
                Ok(StandaloneAgentHistoryEntry {
                    sequence: to_u64(row.get::<i64, _>("sequence"))?,
                    session_id,
                    agent: row
                        .get::<String, _>("agent_kind")
                        .parse::<StandaloneAgentKind>()?,
                    role: row.get("role"),
                    content: row.get("content"),
                    created_at: row.get("created_at"),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let next = entries.last().map_or(after, |entry| entry.sequence);
        Ok(StandaloneAgentHistoryPage {
            after,
            next,
            entries,
            has_more,
        })
    }

    pub async fn refresh_context(&self, session: &Session) -> Result<PathBuf> {
        let path = self.context_path(session.id);
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("standalone agent context path has no parent"))?;
        tokio::fs::create_dir_all(parent)
            .await
            .context("create standalone agent context directory")?;
        tokio::fs::write(&path, render_context(session))
            .await
            .context("write standalone agent shared context")?;
        Ok(path)
    }

    pub async fn context(&self, session: &Session) -> Result<(String, String)> {
        let path = self.refresh_context(session).await?;
        let content = tokio::fs::read_to_string(&path)
            .await
            .context("read standalone agent shared context")?;
        Ok((path.display().to_string(), content))
    }

    fn context_path(&self, session_id: SessionId) -> PathBuf {
        self.workspace_root
            .join(".sessionweft")
            .join("sessions")
            .join(session_id.to_string())
            .join("context.md")
    }

    async fn write_active_prompt(
        &self,
        session_id: SessionId,
        agent: StandaloneAgentKind,
        message: &str,
    ) -> Result<()> {
        let directory = self
            .context_path(session_id)
            .parent()
            .ok_or_else(|| anyhow!("standalone agent prompt path has no parent"))?
            .to_owned();
        tokio::fs::create_dir_all(&directory).await?;
        let content = format!(
            "# Active SessionWeft prompt\n\nAgent: {agent}\nSession: {session_id}\nUpdated: {}\n\n{message}\n",
            Utc::now().to_rfc3339()
        );
        tokio::fs::write(directory.join("active-prompt.md"), content).await?;
        Ok(())
    }

    fn spawn_capture(
        &self,
        runtime: StandaloneRuntime,
        session_id: SessionId,
        agent: StandaloneAgentKind,
        pty_id: Uuid,
        mut cursor: u64,
    ) {
        if let Ok(mut tasks) = self.capture_tasks.lock() {
            tasks.insert((session_id, agent), pty_id);
        }
        let manager = self.clone();
        tokio::spawn(async move {
            loop {
                let still_current = manager
                    .capture_tasks
                    .lock()
                    .ok()
                    .and_then(|tasks| tasks.get(&(session_id, agent)).copied())
                    == Some(pty_id);
                if !still_current {
                    break;
                }
                let batch = match manager
                    .pty
                    .wait_for_output(pty_id, cursor, Duration::from_secs(1))
                    .await
                {
                    Ok(batch) => batch,
                    Err(PtyError::NotFound(_)) => {
                        let _ = manager
                            .update_binding_progress(session_id, agent, cursor, "resume_required")
                            .await;
                        break;
                    }
                    Err(error) => {
                        let _ = manager
                            .append_history(
                                session_id,
                                agent,
                                "system",
                                &format!("output capture failed: {error}"),
                            )
                            .await;
                        break;
                    }
                };
                cursor = batch.next;
                let raw = batch
                    .chunks
                    .iter()
                    .map(|chunk| chunk.data.as_str())
                    .collect::<String>();
                let output = sanitize_terminal_output(&raw);
                if !output.is_empty() {
                    let _ = manager
                        .append_history(session_id, agent, "assistant", &output)
                        .await;
                    if let Ok(session) = append_shared_message(
                        &runtime,
                        session_id,
                        MessageRole::Assistant,
                        format!("[{agent}]\n{output}"),
                        &format!("standalone-agent:{agent}"),
                    )
                    .await
                    {
                        let _ = manager.refresh_context(&session).await;
                    }
                }
                let status = match batch.status {
                    PtyStatus::Running => "running",
                    PtyStatus::Exited => "exited",
                    PtyStatus::Cancelled => "stopped",
                    PtyStatus::Failed => "failed",
                };
                let _ = manager
                    .update_binding_progress(session_id, agent, cursor, status)
                    .await;
                if batch.status != PtyStatus::Running {
                    break;
                }
            }
        });
    }

    async fn binding_view(
        &self,
        row: BindingRow,
        active: Option<StandaloneAgentKind>,
    ) -> Result<StandaloneAgentBindingView> {
        let mut status = row.status.clone();
        let resume_required;
        if let Some(pty_id) = row.pty_id {
            match self.pty.descriptor(pty_id) {
                Ok(descriptor) => {
                    status = match descriptor.status {
                        PtyStatus::Running => "running",
                        PtyStatus::Exited => "exited",
                        PtyStatus::Cancelled => "stopped",
                        PtyStatus::Failed => "failed",
                    }
                    .into();
                    resume_required = descriptor.status != PtyStatus::Running;
                }
                Err(PtyError::NotFound(_)) => {
                    status = "resume_required".into();
                    resume_required = true;
                }
                Err(error) => return Err(error.into()),
            }
        } else {
            resume_required = true;
        }
        Ok(StandaloneAgentBindingView {
            session_id: row.session_id,
            agent: row.agent,
            program: row.program,
            cwd: row.cwd,
            args: row.args,
            pty_id: row.pty_id,
            status,
            active: active == Some(row.agent),
            resume_required,
            direct_input_supported: row.agent.direct_input_supported(),
            output_cursor: row.output_cursor,
            context_path: self.context_path(row.session_id).display().to_string(),
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }

    async fn binding_row(
        &self,
        session_id: SessionId,
        agent: StandaloneAgentKind,
    ) -> Result<Option<BindingRow>> {
        let row = sqlx::query(
            r#"
            SELECT session_id, agent_kind, program, cwd, args_json, pty_id,
                   output_cursor, status, created_at, updated_at
            FROM standalone_agent_bindings
            WHERE session_id = ? AND agent_kind = ?
            "#,
        )
        .bind(session_id.to_string())
        .bind(agent.to_string())
        .fetch_optional(&self.pool)
        .await
        .context("read standalone agent binding")?;
        row.map(binding_from_row).transpose()
    }

    async fn list_binding_rows(&self, session_id: SessionId) -> Result<Vec<BindingRow>> {
        sqlx::query(
            r#"
            SELECT session_id, agent_kind, program, cwd, args_json, pty_id,
                   output_cursor, status, created_at, updated_at
            FROM standalone_agent_bindings
            WHERE session_id = ?
            ORDER BY agent_kind ASC
            "#,
        )
        .bind(session_id.to_string())
        .fetch_all(&self.pool)
        .await
        .context("list standalone agent bindings")?
        .into_iter()
        .map(binding_from_row)
        .collect()
    }

    #[allow(clippy::too_many_arguments)]
    async fn upsert_binding(
        &self,
        session_id: SessionId,
        agent: StandaloneAgentKind,
        program: &str,
        cwd: &str,
        args: &[String],
        pty_id: Uuid,
        output_cursor: u64,
        status: &str,
        created_at: &str,
        updated_at: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO standalone_agent_bindings (
                session_id, agent_kind, program, cwd, args_json, pty_id,
                output_cursor, status, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (session_id, agent_kind) DO UPDATE SET
                program = excluded.program,
                cwd = excluded.cwd,
                args_json = excluded.args_json,
                pty_id = excluded.pty_id,
                output_cursor = excluded.output_cursor,
                status = excluded.status,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(session_id.to_string())
        .bind(agent.to_string())
        .bind(program)
        .bind(cwd)
        .bind(serde_json::to_string(args)?)
        .bind(pty_id.to_string())
        .bind(to_i64(output_cursor)?)
        .bind(status)
        .bind(created_at)
        .bind(updated_at)
        .execute(&self.pool)
        .await
        .context("persist standalone agent binding")?;
        Ok(())
    }

    async fn update_binding_progress(
        &self,
        session_id: SessionId,
        agent: StandaloneAgentKind,
        output_cursor: u64,
        status: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE standalone_agent_bindings
            SET output_cursor = ?, status = ?, updated_at = ?
            WHERE session_id = ? AND agent_kind = ?
            "#,
        )
        .bind(to_i64(output_cursor)?)
        .bind(status)
        .bind(Utc::now().to_rfc3339())
        .bind(session_id.to_string())
        .bind(agent.to_string())
        .execute(&self.pool)
        .await
        .context("update standalone agent binding")?;
        Ok(())
    }

    async fn set_active(&self, session_id: SessionId, agent: StandaloneAgentKind) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO standalone_agent_session_state (session_id, active_agent_kind, updated_at)
            VALUES (?, ?, ?)
            ON CONFLICT (session_id) DO UPDATE SET
                active_agent_kind = excluded.active_agent_kind,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(session_id.to_string())
        .bind(agent.to_string())
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .context("select active standalone agent")?;
        Ok(())
    }

    async fn active_agent(&self, session_id: SessionId) -> Result<Option<StandaloneAgentKind>> {
        let value = sqlx::query_scalar::<_, Option<String>>(
            "SELECT active_agent_kind FROM standalone_agent_session_state WHERE session_id = ?",
        )
        .bind(session_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .context("read active standalone agent")?
        .flatten();
        value.map(|value| value.parse()).transpose()
    }

    async fn append_history(
        &self,
        session_id: SessionId,
        agent: StandaloneAgentKind,
        role: &str,
        content: &str,
    ) -> Result<()> {
        let content = if content.len() > 1_000_000 {
            &content[content.len() - 1_000_000..]
        } else {
            content
        };
        sqlx::query(
            r#"
            INSERT INTO standalone_agent_history (
                session_id, agent_kind, role, content, created_at
            ) VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(session_id.to_string())
        .bind(agent.to_string())
        .bind(role)
        .bind(content)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .context("append standalone agent history")?;
        Ok(())
    }
}

pub async fn build_runtime(database_url: &str) -> Result<StandaloneRuntime> {
    let repository = Arc::new(
        SqliteSessionRepository::connect(database_url)
            .await
            .context("initialize standalone agent Session repository")?,
    );
    Ok(RuntimeService::new(
        repository,
        Arc::new(ProviderRegistry::new()),
    ))
}

async fn append_shared_message(
    runtime: &StandaloneRuntime,
    session_id: SessionId,
    role: MessageRole,
    content: String,
    actor_id: &str,
) -> Result<Session> {
    for _ in 0..5 {
        let session = runtime.get_session(session_id).await?;
        match runtime
            .append_message(
                session_id,
                session.version,
                role,
                content.clone(),
                Some(actor_id),
                Uuid::new_v4(),
            )
            .await
        {
            Ok(session) => return Ok(session),
            Err(RuntimeError::Domain(DomainError::Conflict { .. }))
            | Err(RuntimeError::Storage(StorageError::Conflict { .. })) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    bail!("could not append standalone agent message after concurrent Session updates")
}

fn binding_from_row(row: sqlx::sqlite::SqliteRow) -> Result<BindingRow> {
    let session_id = row
        .get::<String, _>("session_id")
        .parse::<SessionId>()
        .context("parse standalone agent session ID")?;
    let agent = row
        .get::<String, _>("agent_kind")
        .parse::<StandaloneAgentKind>()?;
    let pty_id = row
        .get::<Option<String>, _>("pty_id")
        .map(|value| Uuid::parse_str(&value))
        .transpose()
        .context("parse standalone agent PTY ID")?;
    Ok(BindingRow {
        session_id,
        agent,
        program: row.get("program"),
        cwd: row.get("cwd"),
        args: serde_json::from_str(row.get::<String, _>("args_json").as_str())
            .context("parse standalone agent arguments")?,
        pty_id,
        output_cursor: to_u64(row.get("output_cursor"))?,
        status: row.get("status"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn forwarded_environment() -> BTreeMap<String, String> {
    [
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "PATH",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "TERM",
        "COLORTERM",
        "LANG",
        "LC_ALL",
    ]
    .into_iter()
    .filter_map(|key| env::var(key).ok().map(|value| (key.into(), value)))
    .collect()
}

fn render_context(session: &Session) -> String {
    let mut output = format!(
        "# SessionWeft shared context\n\n- Session ID: `{}`\n- Title: {}\n- Version: {}\n- Updated: {}\n\n## Shared chat history\n",
        session.id, session.title, session.version, session.updated_at
    );
    let start = session.messages.len().saturating_sub(200);
    for message in &session.messages[start..] {
        output.push_str(&format!(
            "\n### {} · {}\n\n{}\n",
            message.role, message.created_at, message.content
        ));
    }
    output.push_str(
        "\n## Continuation rule\n\nAll standalone agents operate on this same Session. Preserve prior decisions, do not reset context when switching agents, and record new work back into the shared history.\n",
    );
    output
}

fn sanitize_terminal_output(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() || next == '~' {
                        break;
                    }
                }
            } else {
                chars.next();
            }
            continue;
        }
        if character != '\r' && character != '\0' {
            output.push(character);
        }
    }
    let trimmed = output.trim();
    if trimmed.len() > 1_000_000 {
        trimmed[trimmed.len() - 1_000_000..].to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn to_i64(value: u64) -> Result<i64> {
    i64::try_from(value).context("standalone agent cursor exceeds i64")
}

fn to_u64(value: i64) -> Result<u64> {
    u64::try_from(value).context("standalone agent cursor cannot be negative")
}

#[cfg(test)]
mod tests {
    use sessionweft_core::Session;

    use super::*;

    #[test]
    fn agent_aliases_are_stable() {
        assert_eq!(
            "codex".parse::<StandaloneAgentKind>().unwrap(),
            StandaloneAgentKind::Codex
        );
        assert_eq!(
            "claude-code".parse::<StandaloneAgentKind>().unwrap(),
            StandaloneAgentKind::Claude
        );
        assert_eq!(
            "gemini_cli".parse::<StandaloneAgentKind>().unwrap(),
            StandaloneAgentKind::Gemini
        );
        assert_eq!(
            "anti".parse::<StandaloneAgentKind>().unwrap(),
            StandaloneAgentKind::AntigravityIde
        );
    }

    #[test]
    fn terminal_output_removes_ansi_control_sequences() {
        assert_eq!(
            sanitize_terminal_output("\u{1b}[31mhello\u{1b}[0m\r\n"),
            "hello"
        );
    }

    #[test]
    fn shared_context_contains_session_history() {
        let session = Session::new("shared").unwrap();
        let context = render_context(&session);
        assert!(context.contains("SessionWeft shared context"));
        assert!(context.contains(&session.id.to_string()));
    }
}
