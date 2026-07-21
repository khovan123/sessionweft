use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sessionweft_core::{EventEnvelope, SessionId};
use sessionweft_orchestration::LockResource;
use thiserror::Error;
use tokio::process::Command;
use uuid::Uuid;

pub const AGENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Planner,
    Architect,
    Worker,
    Reviewer,
    Tester,
    Merger,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Capability {
    Provider,
    WorkspaceRead,
    WorkspaceWrite,
    GitRead,
    GitWrite,
    Terminal,
    Network,
    SecretRead,
    Tool(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentManifest {
    pub name: String,
    pub role: AgentRole,
    pub capabilities: BTreeSet<Capability>,
    pub heartbeat_timeout_seconds: u32,
}

impl AgentManifest {
    pub fn validate(&self) -> Result<(), ExecutionError> {
        if self.name.trim().is_empty() {
            return Err(ExecutionError::Validation(
                "agent name cannot be empty".into(),
            ));
        }
        if self.name.len() > 128 {
            return Err(ExecutionError::Validation(
                "agent name cannot exceed 128 bytes".into(),
            ));
        }
        if !(5..=3_600).contains(&self.heartbeat_timeout_seconds) {
            return Err(ExecutionError::Validation(
                "heartbeat timeout must be between 5 and 3600 seconds".into(),
            ));
        }
        for capability in &self.capabilities {
            if let Capability::Tool(name) = capability
                && name.trim().is_empty()
            {
                return Err(ExecutionError::Validation(
                    "tool capability name cannot be empty".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Registered,
    Running,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRecord {
    pub schema_version: u32,
    pub id: Uuid,
    pub session_id: SessionId,
    pub version: u64,
    pub manifest: AgentManifest,
    pub status: AgentStatus,
    pub heartbeat_at: DateTime<Utc>,
    pub current_task_id: Option<String>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AgentRecord {
    pub fn new(session_id: SessionId, manifest: AgentManifest) -> Result<Self, ExecutionError> {
        manifest.validate()?;
        let now = Utc::now();
        Ok(Self {
            schema_version: AGENT_SCHEMA_VERSION,
            id: Uuid::new_v4(),
            session_id,
            version: 0,
            manifest,
            status: AgentStatus::Registered,
            heartbeat_at: now,
            current_task_id: None,
            last_error: None,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn start(
        &mut self,
        expected_version: u64,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<EventEnvelope, ExecutionError> {
        self.ensure_version(expected_version)?;
        if !matches!(self.status, AgentStatus::Registered | AgentStatus::Stopped) {
            return Err(ExecutionError::InvalidTransition(
                "only registered or stopped agents can start".into(),
            ));
        }
        self.status = AgentStatus::Running;
        self.last_error = None;
        self.heartbeat_at = Utc::now();
        self.advance();
        Ok(self.event("agent.started", correlation_id, actor_id, json!({})))
    }

    pub fn heartbeat(
        &mut self,
        expected_version: u64,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<EventEnvelope, ExecutionError> {
        self.ensure_running(expected_version)?;
        self.heartbeat_at = Utc::now();
        self.advance();
        Ok(self.event(
            "agent.heartbeat",
            correlation_id,
            actor_id,
            json!({"heartbeat_at": self.heartbeat_at}),
        ))
    }

    pub fn assign_task(
        &mut self,
        expected_version: u64,
        task_id: impl Into<String>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<EventEnvelope, ExecutionError> {
        self.ensure_running(expected_version)?;
        if self.current_task_id.is_some() {
            return Err(ExecutionError::InvalidTransition(
                "agent already owns a task".into(),
            ));
        }
        let task_id = task_id.into().trim().to_owned();
        if task_id.is_empty() {
            return Err(ExecutionError::Validation(
                "task ID cannot be empty".into(),
            ));
        }
        self.current_task_id = Some(task_id.clone());
        self.advance();
        Ok(self.event(
            "agent.task_assigned",
            correlation_id,
            actor_id,
            json!({"task_id": task_id}),
        ))
    }

    pub fn release_task(
        &mut self,
        expected_version: u64,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<EventEnvelope, ExecutionError> {
        self.ensure_running(expected_version)?;
        let task_id = self.current_task_id.take().ok_or_else(|| {
            ExecutionError::InvalidTransition("agent does not own a task".into())
        })?;
        self.advance();
        Ok(self.event(
            "agent.task_released",
            correlation_id,
            actor_id,
            json!({"task_id": task_id}),
        ))
    }

    pub fn fail(
        &mut self,
        expected_version: u64,
        sanitized_error: impl Into<String>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<EventEnvelope, ExecutionError> {
        self.ensure_version(expected_version)?;
        let error = sanitized_error.into();
        self.status = AgentStatus::Failed;
        self.current_task_id = None;
        self.last_error = Some(error.clone());
        self.advance();
        Ok(self.event(
            "agent.failed",
            correlation_id,
            actor_id,
            json!({"error": error}),
        ))
    }

    pub fn stop(
        &mut self,
        expected_version: u64,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<EventEnvelope, ExecutionError> {
        self.ensure_running(expected_version)?;
        if self.current_task_id.is_some() {
            return Err(ExecutionError::InvalidTransition(
                "release the current task before stopping the agent".into(),
            ));
        }
        self.status = AgentStatus::Stopped;
        self.advance();
        Ok(self.event("agent.stopped", correlation_id, actor_id, json!({})))
    }

    #[must_use]
    pub fn is_stale_at(&self, now: DateTime<Utc>) -> bool {
        self.status == AgentStatus::Running
            && (now - self.heartbeat_at).num_seconds()
                > i64::from(self.manifest.heartbeat_timeout_seconds)
    }

    #[must_use]
    pub fn allows(&self, permission: &Permission) -> bool {
        let capability = match permission {
            Permission::WorkspaceRead => Capability::WorkspaceRead,
            Permission::WorkspaceWrite => Capability::WorkspaceWrite,
            Permission::GitRead => Capability::GitRead,
            Permission::GitWrite => Capability::GitWrite,
            Permission::Terminal => Capability::Terminal,
            Permission::Network => Capability::Network,
            Permission::SecretRead => Capability::SecretRead,
            Permission::Tool(name) => Capability::Tool(name.clone()),
        };
        self.manifest.capabilities.contains(&capability)
    }

    fn ensure_version(&self, expected_version: u64) -> Result<(), ExecutionError> {
        if self.version != expected_version {
            return Err(ExecutionError::VersionConflict {
                expected: expected_version,
                actual: self.version,
            });
        }
        Ok(())
    }

    fn ensure_running(&self, expected_version: u64) -> Result<(), ExecutionError> {
        self.ensure_version(expected_version)?;
        if self.status != AgentStatus::Running {
            return Err(ExecutionError::InvalidTransition(
                "agent is not running".into(),
            ));
        }
        Ok(())
    }

    fn advance(&mut self) {
        self.version = self.version.saturating_add(1);
        self.updated_at = Utc::now();
    }

    fn event(
        &self,
        event_type: &str,
        correlation_id: Uuid,
        actor_id: Option<&str>,
        details: Value,
    ) -> EventEnvelope {
        EventEnvelope::new(
            event_type,
            Some(self.session_id),
            correlation_id,
            actor_id,
            json!({
                "agent_id": self.id,
                "agent_version": self.version,
                "status": self.status,
                "role": self.manifest.role,
                "details": details,
            }),
        )
    }
}

#[async_trait]
pub trait AgentRepository: Send + Sync {
    async fn create(
        &self,
        agent: &AgentRecord,
        events: &[EventEnvelope],
    ) -> Result<AgentRecord, RepositoryError>;
    async fn get(&self, agent_id: Uuid) -> Result<Option<AgentRecord>, RepositoryError>;
    async fn save(
        &self,
        expected_version: u64,
        agent: &AgentRecord,
        events: &[EventEnvelope],
    ) -> Result<AgentRecord, RepositoryError>;
    async fn stale_agents(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<AgentRecord>, RepositoryError>;
}

#[derive(Clone)]
pub struct AgentService<R>
where
    R: AgentRepository,
{
    repository: Arc<R>,
}

impl<R> AgentService<R>
where
    R: AgentRepository,
{
    #[must_use]
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    pub async fn register(
        &self,
        session_id: SessionId,
        manifest: AgentManifest,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<AgentRecord, ExecutionError> {
        let agent = AgentRecord::new(session_id, manifest)?;
        let event = agent.event("agent.registered", correlation_id, actor_id, json!({}));
        self.repository
            .create(&agent, &[event])
            .await
            .map_err(ExecutionError::Repository)
    }

    pub async fn get(&self, agent_id: Uuid) -> Result<AgentRecord, ExecutionError> {
        self.repository
            .get(agent_id)
            .await
            .map_err(ExecutionError::Repository)?
            .ok_or(ExecutionError::AgentNotFound(agent_id))
    }

    pub async fn mutate<F>(
        &self,
        agent_id: Uuid,
        expected_version: u64,
        operation: F,
    ) -> Result<AgentRecord, ExecutionError>
    where
        F: FnOnce(&mut AgentRecord) -> Result<Vec<EventEnvelope>, ExecutionError> + Send,
    {
        let mut agent = self.get(agent_id).await?;
        let events = operation(&mut agent)?;
        self.repository
            .save(expected_version, &agent, &events)
            .await
            .map_err(ExecutionError::Repository)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Permission {
    WorkspaceRead,
    WorkspaceWrite,
    GitRead,
    GitWrite,
    Terminal,
    Network,
    SecretRead,
    Tool(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub version: String,
    pub permissions: BTreeSet<Permission>,
    pub risk: RiskLevel,
    pub input_schema: Value,
}

impl ToolDescriptor {
    pub fn validate(&self) -> Result<(), ExecutionError> {
        if self.name.trim().is_empty() || self.version.trim().is_empty() {
            return Err(ExecutionError::Validation(
                "tool name and version are required".into(),
            ));
        }
        if !self.input_schema.is_object() {
            return Err(ExecutionError::Validation(
                "tool input schema must be a JSON object".into(),
            ));
        }
        if !self
            .permissions
            .contains(&Permission::Tool(self.name.clone()))
        {
            return Err(ExecutionError::Validation(
                "tool descriptor must include its self-named Tool permission".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyConfig {
    pub allowed: BTreeSet<Permission>,
    pub approval_required: BTreeSet<Permission>,
    pub denied: BTreeSet<Permission>,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            allowed: BTreeSet::new(),
            approval_required: BTreeSet::new(),
            denied: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffect {
    Allow,
    Deny,
    RequireApproval,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub effect: PolicyEffect,
    pub reason: String,
}

#[derive(Clone)]
pub struct PolicyEngine {
    config: Arc<PolicyConfig>,
}

impl PolicyEngine {
    #[must_use]
    pub fn new(config: PolicyConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }

    #[must_use]
    pub fn evaluate(&self, agent: &AgentRecord, tool: &ToolDescriptor) -> PolicyDecision {
        for permission in &tool.permissions {
            if !agent.allows(permission) {
                return PolicyDecision {
                    effect: PolicyEffect::Deny,
                    reason: format!("agent lacks permission {permission:?}"),
                };
            }
            if self.config.denied.contains(permission) {
                return PolicyDecision {
                    effect: PolicyEffect::Deny,
                    reason: format!("permission {permission:?} is explicitly denied"),
                };
            }
            if self.config.approval_required.contains(permission) {
                return PolicyDecision {
                    effect: PolicyEffect::RequireApproval,
                    reason: format!("permission {permission:?} requires approval"),
                };
            }
            if !self.config.allowed.contains(permission) {
                return PolicyDecision {
                    effect: PolicyEffect::Deny,
                    reason: format!("permission {permission:?} is not allowlisted"),
                };
            }
        }
        if matches!(tool.risk, RiskLevel::High | RiskLevel::Critical) {
            return PolicyDecision {
                effect: PolicyEffect::RequireApproval,
                reason: "high-risk tool requires approval".into(),
            };
        }
        PolicyDecision {
            effect: PolicyEffect::Allow,
            reason: "all required permissions are allowed".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalGrant {
    pub id: Uuid,
    pub session_id: SessionId,
    pub agent_id: Uuid,
    pub tool_name: String,
    pub expires_at: DateTime<Utc>,
}

impl ApprovalGrant {
    #[must_use]
    pub fn authorizes(
        &self,
        session_id: SessionId,
        agent_id: Uuid,
        tool_name: &str,
        now: DateTime<Utc>,
    ) -> bool {
        self.session_id == session_id
            && self.agent_id == agent_id
            && self.tool_name == tool_name
            && self.expires_at > now
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub session_id: SessionId,
    pub task_id: Option<String>,
    pub agent_id: Uuid,
    pub tool_name: String,
    pub arguments: Value,
    pub correlation_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: Value,
    pub metadata: BTreeMap<String, String>,
}

#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn descriptor(&self) -> &ToolDescriptor;
    async fn invoke(&self, invocation: &ToolInvocation) -> Result<ToolResult, ToolError>;
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ToolHandler>>,
}

impl ToolRegistry {
    pub fn register<T>(&mut self, tool: T) -> Result<(), ExecutionError>
    where
        T: ToolHandler + 'static,
    {
        tool.descriptor().validate()?;
        let name = tool.descriptor().name.clone();
        if self.tools.insert(name.clone(), Arc::new(tool)).is_some() {
            return Err(ExecutionError::Validation(format!(
                "tool '{name}' is already registered"
            )));
        }
        Ok(())
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        self.tools.get(name).cloned()
    }
}

pub struct ToolGateway {
    registry: Arc<ToolRegistry>,
    policy: PolicyEngine,
}

impl ToolGateway {
    #[must_use]
    pub fn new(registry: Arc<ToolRegistry>, policy: PolicyEngine) -> Self {
        Self { registry, policy }
    }

    pub async fn invoke(
        &self,
        agent: &AgentRecord,
        invocation: &ToolInvocation,
        approval: Option<&ApprovalGrant>,
    ) -> Result<ToolResult, ToolError> {
        if invocation.session_id != agent.session_id || invocation.agent_id != agent.id {
            return Err(ToolError::Denied("agent/session scope mismatch".into()));
        }
        let tool = self
            .registry
            .get(&invocation.tool_name)
            .ok_or_else(|| ToolError::NotFound(invocation.tool_name.clone()))?;
        let decision = self.policy.evaluate(agent, tool.descriptor());
        match decision.effect {
            PolicyEffect::Allow => {}
            PolicyEffect::Deny => return Err(ToolError::Denied(decision.reason)),
            PolicyEffect::RequireApproval => {
                let authorized = approval.is_some_and(|grant| {
                    grant.authorizes(
                        invocation.session_id,
                        invocation.agent_id,
                        &invocation.tool_name,
                        Utc::now(),
                    )
                });
                if !authorized {
                    return Err(ToolError::ApprovalRequired(decision.reason));
                }
            }
        }
        tool.invoke(invocation).await
    }
}

#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn list_tools(&self) -> Result<Vec<ToolDescriptor>, ToolError>;
    async fn call_tool(&self, invocation: &ToolInvocation) -> Result<ToolResult, ToolError>;
}

pub struct McpGateway<T>
where
    T: McpTransport,
{
    transport: Arc<T>,
    policy: PolicyEngine,
}

impl<T> McpGateway<T>
where
    T: McpTransport,
{
    #[must_use]
    pub fn new(transport: Arc<T>, policy: PolicyEngine) -> Self {
        Self { transport, policy }
    }

    pub async fn discover(&self) -> Result<Vec<ToolDescriptor>, ToolError> {
        let tools = self.transport.list_tools().await?;
        for tool in &tools {
            tool.validate()
                .map_err(|error| ToolError::InvalidDescriptor(error.to_string()))?;
        }
        Ok(tools)
    }

    pub async fn invoke(
        &self,
        agent: &AgentRecord,
        invocation: &ToolInvocation,
        approval: Option<&ApprovalGrant>,
    ) -> Result<ToolResult, ToolError> {
        if invocation.session_id != agent.session_id || invocation.agent_id != agent.id {
            return Err(ToolError::Denied("agent/session scope mismatch".into()));
        }
        let descriptor = self
            .discover()
            .await?
            .into_iter()
            .find(|tool| tool.name == invocation.tool_name)
            .ok_or_else(|| ToolError::NotFound(invocation.tool_name.clone()))?;
        let decision = self.policy.evaluate(agent, &descriptor);
        match decision.effect {
            PolicyEffect::Allow => self.transport.call_tool(invocation).await,
            PolicyEffect::Deny => Err(ToolError::Denied(decision.reason)),
            PolicyEffect::RequireApproval => {
                let authorized = approval.is_some_and(|grant| {
                    grant.authorizes(
                        invocation.session_id,
                        invocation.agent_id,
                        &invocation.tool_name,
                        Utc::now(),
                    )
                });
                if authorized {
                    self.transport.call_tool(invocation).await
                } else {
                    Err(ToolError::ApprovalRequired(decision.reason))
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone)]
pub struct RestrictedProcessRunner {
    workspace_root: PathBuf,
    programs: Arc<BTreeMap<String, PathBuf>>,
    allowed_env: Arc<BTreeSet<String>>,
}

impl RestrictedProcessRunner {
    pub fn new(
        workspace_root: impl AsRef<Path>,
        programs: BTreeMap<String, PathBuf>,
        allowed_env: BTreeSet<String>,
    ) -> Result<Self, ExecutionError> {
        let workspace_root = fs::canonicalize(workspace_root).map_err(ExecutionError::Io)?;
        if !workspace_root.is_dir() {
            return Err(ExecutionError::Validation(
                "process workspace root must be a directory".into(),
            ));
        }
        let mut canonical_programs = BTreeMap::new();
        for (name, path) in programs {
            if name.trim().is_empty() {
                return Err(ExecutionError::Validation(
                    "program alias cannot be empty".into(),
                ));
            }
            let canonical = fs::canonicalize(path).map_err(ExecutionError::Io)?;
            if !canonical.is_file() {
                return Err(ExecutionError::Validation(format!(
                    "program '{name}' is not a file"
                )));
            }
            canonical_programs.insert(name, canonical);
        }
        Ok(Self {
            workspace_root,
            programs: Arc::new(canonical_programs),
            allowed_env: Arc::new(allowed_env),
        })
    }

    pub async fn run(&self, spec: &ProcessSpec) -> Result<ProcessOutput, ProcessError> {
        if spec.max_output_bytes == 0 || spec.max_output_bytes > 16 * 1024 * 1024 {
            return Err(ProcessError::InvalidSpec(
                "max output must be between 1 byte and 16 MiB".into(),
            ));
        }
        if spec.timeout.is_zero() || spec.timeout > Duration::from_secs(3_600) {
            return Err(ProcessError::InvalidSpec(
                "timeout must be between 1 ms and 1 hour".into(),
            ));
        }
        let program = self
            .programs
            .get(&spec.program)
            .ok_or_else(|| ProcessError::ProgramDenied(spec.program.clone()))?;
        let cwd = fs::canonicalize(&spec.cwd).map_err(ProcessError::Io)?;
        if !cwd.starts_with(&self.workspace_root) {
            return Err(ProcessError::WorkspaceEscape(cwd));
        }
        for key in spec.env.keys() {
            if !self.allowed_env.contains(key) {
                return Err(ProcessError::EnvironmentDenied(key.clone()));
            }
        }

        let mut command = Command::new(program);
        command
            .args(&spec.args)
            .current_dir(cwd)
            .env_clear()
            .envs(&spec.env)
            .kill_on_drop(true);
        let output = tokio::time::timeout(spec.timeout, command.output())
            .await
            .map_err(|_| ProcessError::Timeout(spec.timeout))?
            .map_err(ProcessError::Io)?;
        let output_size = output.stdout.len().saturating_add(output.stderr.len());
        if output_size > spec.max_output_bytes {
            return Err(ProcessError::OutputLimitExceeded {
                actual: output_size,
                limit: spec.max_output_bytes,
            });
        }
        Ok(ProcessOutput {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitFence {
    pub owner_id: String,
    pub fencing_token: u64,
    pub resource: LockResource,
}

#[async_trait]
pub trait FenceValidator: Send + Sync {
    async fn validate(&self, fence: &GitFence) -> Result<(), GitError>;
}

#[derive(Clone)]
pub struct GitCli<F>
where
    F: FenceValidator,
{
    runner: RestrictedProcessRunner,
    fence_validator: Arc<F>,
}

impl<F> GitCli<F>
where
    F: FenceValidator,
{
    #[must_use]
    pub fn new(runner: RestrictedProcessRunner, fence_validator: Arc<F>) -> Self {
        Self {
            runner,
            fence_validator,
        }
    }

    pub async fn status(&self) -> Result<String, GitError> {
        self.run_git(["status", "--porcelain=v1", "--untracked-files=all"], None)
            .await
    }

    pub async fn diff(&self) -> Result<String, GitError> {
        self.run_git(["diff", "--no-ext-diff", "--"], None).await
    }

    pub async fn commit_staged(
        &self,
        message: &str,
        fence: &GitFence,
    ) -> Result<String, GitError> {
        if message.trim().is_empty() || message.len() > 1_000 {
            return Err(GitError::InvalidInput(
                "commit message must be between 1 and 1000 bytes".into(),
            ));
        }
        self.fence_validator.validate(fence).await?;
        self.run_git(
            ["commit", "--no-gpg-sign", "--no-verify", "-m", message],
            Some(fence),
        )
        .await
    }

    pub async fn create_worktree(
        &self,
        relative_path: &str,
        branch: &str,
        fence: &GitFence,
    ) -> Result<String, GitError> {
        if relative_path.contains("..") || Path::new(relative_path).is_absolute() {
            return Err(GitError::InvalidInput(
                "worktree path must remain relative to the workspace".into(),
            ));
        }
        if branch.trim().is_empty() || branch.starts_with('-') {
            return Err(GitError::InvalidInput("invalid branch name".into()));
        }
        self.fence_validator.validate(fence).await?;
        self.run_git(
            ["worktree", "add", "-b", branch, "--", relative_path],
            Some(fence),
        )
        .await
    }

    async fn run_git<const N: usize>(
        &self,
        args: [&str; N],
        _fence: Option<&GitFence>,
    ) -> Result<String, GitError> {
        let output = self
            .runner
            .run(&ProcessSpec {
                program: "git".into(),
                args: args.into_iter().map(ToOwned::to_owned).collect(),
                cwd: self.runner.workspace_root().to_owned(),
                env: BTreeMap::new(),
                timeout: Duration::from_secs(120),
                max_output_bytes: 4 * 1024 * 1024,
            })
            .await
            .map_err(GitError::Process)?;
        if output.exit_code == Some(0) {
            Ok(output.stdout)
        } else {
            Err(GitError::CommandFailed {
                exit_code: output.exit_code,
                stderr: output.stderr,
            })
        }
    }
}

pub fn find_executable(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| fs::canonicalize(candidate).ok())
}

#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("agent {0} not found")]
    AgentNotFound(Uuid),
    #[error("agent version conflict: expected {expected}, actual {actual}")]
    VersionConflict { expected: u64, actual: u64 },
    #[error("invalid agent transition: {0}")]
    InvalidTransition(String),
    #[error("agent repository error: {0}")]
    Repository(RepositoryError),
    #[error("I/O error: {0}")]
    Io(std::io::Error),
}

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("backend error: {0}")]
    Backend(String),
    #[error("agent {0} not found")]
    AgentNotFound(Uuid),
    #[error("agent version conflict: expected {expected}, actual {actual}")]
    VersionConflict { expected: u64, actual: u64 },
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("tool '{0}' not found")]
    NotFound(String),
    #[error("tool request denied: {0}")]
    Denied(String),
    #[error("tool approval required: {0}")]
    ApprovalRequired(String),
    #[error("invalid tool descriptor: {0}")]
    InvalidDescriptor(String),
    #[error("tool execution failed: {0}")]
    Execution(String),
}

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("invalid process specification: {0}")]
    InvalidSpec(String),
    #[error("program '{0}' is not allowlisted")]
    ProgramDenied(String),
    #[error("environment variable '{0}' is not allowlisted")]
    EnvironmentDenied(String),
    #[error("process working directory escapes workspace: {0}")]
    WorkspaceEscape(PathBuf),
    #[error("process timed out after {0:?}")]
    Timeout(Duration),
    #[error("process output {actual} bytes exceeds limit {limit}")]
    OutputLimitExceeded { actual: usize, limit: usize },
    #[error("process I/O error: {0}")]
    Io(std::io::Error),
}

#[derive(Debug, Error)]
pub enum GitError {
    #[error("invalid Git input: {0}")]
    InvalidInput(String),
    #[error("Git process error: {0}")]
    Process(ProcessError),
    #[error("Git command failed with {exit_code:?}: {stderr}")]
    CommandFailed {
        exit_code: Option<i32>,
        stderr: String,
    },
    #[error("Git mutation fence rejected: {0}")]
    FenceRejected(String),
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    fn test_agent(capabilities: BTreeSet<Capability>) -> AgentRecord {
        AgentRecord::new(
            SessionId::new(),
            AgentManifest {
                name: "worker".into(),
                role: AgentRole::Worker,
                capabilities,
                heartbeat_timeout_seconds: 30,
            },
        )
        .expect("agent")
    }

    #[test]
    fn agent_lifecycle_is_versioned_and_task_owned() {
        let mut agent = test_agent(BTreeSet::new());
        agent
            .start(0, Uuid::new_v4(), Some("test"))
            .expect("start");
        agent
            .assign_task(1, "task-1", Uuid::new_v4(), Some("test"))
            .expect("assign");
        assert_eq!(agent.version, 2);
        assert_eq!(agent.current_task_id.as_deref(), Some("task-1"));
        assert!(agent.stop(2, Uuid::new_v4(), Some("test")).is_err());
    }

    struct CountingTransport {
        calls: AtomicUsize,
        tool: ToolDescriptor,
    }

    #[async_trait]
    impl McpTransport for CountingTransport {
        async fn list_tools(&self) -> Result<Vec<ToolDescriptor>, ToolError> {
            Ok(vec![self.tool.clone()])
        }

        async fn call_tool(&self, _invocation: &ToolInvocation) -> Result<ToolResult, ToolError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolResult {
                content: json!({"ok": true}),
                metadata: BTreeMap::new(),
            })
        }
    }

    #[tokio::test]
    async fn mcp_transport_cannot_bypass_default_deny_policy() {
        let agent = test_agent(BTreeSet::from([Capability::Tool("remote.echo".into())]));
        let transport = Arc::new(CountingTransport {
            calls: AtomicUsize::new(0),
            tool: ToolDescriptor {
                name: "remote.echo".into(),
                version: "1".into(),
                permissions: BTreeSet::from([Permission::Tool("remote.echo".into())]),
                risk: RiskLevel::Low,
                input_schema: json!({"type": "object"}),
            },
        });
        let gateway = McpGateway::new(
            Arc::clone(&transport),
            PolicyEngine::new(PolicyConfig::default()),
        );
        let error = gateway
            .invoke(
                &agent,
                &ToolInvocation {
                    session_id: agent.session_id,
                    task_id: None,
                    agent_id: agent.id,
                    tool_name: "remote.echo".into(),
                    arguments: json!({}),
                    correlation_id: Uuid::new_v4(),
                },
                None,
            )
            .await
            .expect_err("default deny");
        assert!(matches!(error, ToolError::Denied(_)));
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn restricted_runner_rejects_unlisted_programs() {
        let root = env::temp_dir().join(format!("sessionweft-process-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("root");
        let runner = RestrictedProcessRunner::new(&root, BTreeMap::new(), BTreeSet::new())
            .expect("runner");
        let error = runner
            .run(&ProcessSpec {
                program: "sh".into(),
                args: vec![],
                cwd: root.clone(),
                env: BTreeMap::new(),
                timeout: Duration::from_secs(1),
                max_output_bytes: 1024,
            })
            .await
            .expect_err("program denied");
        assert!(matches!(error, ProcessError::ProgramDenied(_)));
        fs::remove_dir_all(root).expect("cleanup");
    }

    struct AllowFence;

    #[async_trait]
    impl FenceValidator for AllowFence {
        async fn validate(&self, _fence: &GitFence) -> Result<(), GitError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn git_status_uses_restricted_cli_adapter() {
        let Some(git) = find_executable("git") else {
            return;
        };
        let root = env::temp_dir().join(format!("sessionweft-git-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("root");
        let setup = std::process::Command::new(&git)
            .args(["init", "--quiet"])
            .current_dir(&root)
            .status()
            .expect("git init");
        assert!(setup.success());
        fs::write(root.join("new.txt"), "new").expect("file");
        let runner = RestrictedProcessRunner::new(
            &root,
            BTreeMap::from([("git".into(), git)]),
            BTreeSet::new(),
        )
        .expect("runner");
        let git = GitCli::new(runner, Arc::new(AllowFence));
        let status = git.status().await.expect("status");
        assert!(status.contains("new.txt"));
        fs::remove_dir_all(root).expect("cleanup");
    }
}
