use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sessionweft_core::{ProviderMessage, SessionId};
use sessionweft_execution::{RiskLevel, ToolDescriptor};
use sessionweft_orchestration::{WorkflowExecution, WorkflowNodeKind};
use thiserror::Error;
use uuid::Uuid;

use super::RepositoryError;

pub const TASK_EXECUTION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskAction {
    Provider {
        provider: String,
        model: String,
        messages: Vec<ProviderMessage>,
    },
    Tool {
        descriptor: ToolDescriptor,
        input: Value,
    },
}

impl TaskAction {
    pub fn validate(&self) -> Result<(), TaskExecutionError> {
        match self {
            Self::Provider {
                provider,
                model,
                messages,
            } => {
                if provider.trim().is_empty() || model.trim().is_empty() {
                    return Err(TaskExecutionError::Validation(
                        "provider action requires provider and model".into(),
                    ));
                }
                if messages.is_empty() {
                    return Err(TaskExecutionError::Validation(
                        "provider action requires at least one message".into(),
                    ));
                }
                if messages.iter().any(|message| message.content.trim().is_empty()) {
                    return Err(TaskExecutionError::Validation(
                        "provider action messages cannot be empty".into(),
                    ));
                }
            }
            Self::Tool { descriptor, input } => {
                descriptor
                    .validate()
                    .map_err(|error| TaskExecutionError::Validation(error.to_string()))?;
                if !input.is_object() {
                    return Err(TaskExecutionError::Validation(
                        "tool action input must be a JSON object".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn requires_explicit_approval(&self) -> bool {
        matches!(
            self,
            Self::Tool {
                descriptor: ToolDescriptor {
                    risk: RiskLevel::High | RiskLevel::Critical,
                    ..
                },
                ..
            }
        )
    }

    #[must_use]
    pub fn action_name(&self) -> String {
        match self {
            Self::Provider { provider, .. } => format!("provider:{provider}"),
            Self::Tool { descriptor, .. } => format!("tool:{}", descriptor.name),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskExecutionSpec {
    pub schema_version: u32,
    pub workflow_id: Uuid,
    pub session_id: SessionId,
    pub node_id: String,
    pub action: TaskAction,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TaskExecutionSpec {
    pub fn new(
        workflow: &WorkflowExecution,
        node_id: impl Into<String>,
        action: TaskAction,
    ) -> Result<Self, TaskExecutionError> {
        let node_id = node_id.into();
        let is_task = workflow
            .definition
            .nodes
            .iter()
            .any(|node| node.id == node_id && node.kind == WorkflowNodeKind::Task);
        if !is_task {
            return Err(TaskExecutionError::Validation(format!(
                "execution spec references non-task or missing node '{node_id}'"
            )));
        }
        action.validate()?;
        let now = Utc::now();
        Ok(Self {
            schema_version: TASK_EXECUTION_SCHEMA_VERSION,
            workflow_id: workflow.id,
            session_id: workflow.session_id,
            node_id,
            action,
            created_at: now,
            updated_at: now,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskExecutionStatus {
    Prepared,
    Running,
    Succeeded,
    Failed,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskExecutionRecord {
    pub schema_version: u32,
    pub id: Uuid,
    pub claim_id: Uuid,
    pub session_id: SessionId,
    pub workflow_id: Uuid,
    pub node_id: String,
    pub agent_id: Uuid,
    pub idempotency_key: String,
    pub action: TaskAction,
    pub status: TaskExecutionStatus,
    pub output: Option<Value>,
    pub sanitized_error: Option<String>,
    pub prepared_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

impl TaskExecutionRecord {
    #[must_use]
    pub fn prepared(
        claim_id: Uuid,
        session_id: SessionId,
        workflow_id: Uuid,
        node_id: String,
        agent_id: Uuid,
        idempotency_key: String,
        action: TaskAction,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            schema_version: TASK_EXECUTION_SCHEMA_VERSION,
            id: Uuid::new_v4(),
            claim_id,
            session_id,
            workflow_id,
            node_id,
            agent_id,
            idempotency_key,
            action,
            status: TaskExecutionStatus::Prepared,
            output: None,
            sanitized_error: None,
            prepared_at: now,
            started_at: None,
            completed_at: None,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolExecutionApproval {
    pub id: Uuid,
    pub claim_id: Uuid,
    pub session_id: SessionId,
    pub agent_id: Uuid,
    pub tool_name: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl ToolExecutionApproval {
    pub fn new(
        claim_id: Uuid,
        session_id: SessionId,
        agent_id: Uuid,
        tool_name: impl Into<String>,
        expires_at: DateTime<Utc>,
    ) -> Result<Self, TaskExecutionError> {
        let tool_name = tool_name.into().trim().to_owned();
        let now = Utc::now();
        if tool_name.is_empty() {
            return Err(TaskExecutionError::Validation(
                "tool approval requires a tool name".into(),
            ));
        }
        if expires_at <= now {
            return Err(TaskExecutionError::Validation(
                "tool approval expiry must be in the future".into(),
            ));
        }
        Ok(Self {
            id: Uuid::new_v4(),
            claim_id,
            session_id,
            agent_id,
            tool_name,
            expires_at,
            created_at: now,
        })
    }
}

#[async_trait]
pub trait TaskExecutionRepository: Send + Sync {
    async fn register_spec(
        &self,
        spec: &TaskExecutionSpec,
    ) -> Result<TaskExecutionSpec, RepositoryError>;

    async fn get_spec(
        &self,
        workflow_id: Uuid,
        node_id: &str,
    ) -> Result<Option<TaskExecutionSpec>, RepositoryError>;

    async fn grant_tool_approval(
        &self,
        approval: &ToolExecutionApproval,
    ) -> Result<ToolExecutionApproval, RepositoryError>;

    async fn prepare_claim_execution(
        &self,
        claim_id: Uuid,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<Option<TaskExecutionRecord>, RepositoryError>;

    async fn mark_execution_running(
        &self,
        execution_id: Uuid,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<TaskExecutionRecord, RepositoryError>;

    async fn succeed_execution(
        &self,
        execution_id: Uuid,
        output: Value,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<TaskExecutionRecord, RepositoryError>;

    async fn fail_execution(
        &self,
        execution_id: Uuid,
        sanitized_error: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<TaskExecutionRecord, RepositoryError>;

    async fn mark_stale_running_uncertain(
        &self,
        stale_before: DateTime<Utc>,
        limit: usize,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<Vec<TaskExecutionRecord>, RepositoryError>;

    async fn get_execution(
        &self,
        execution_id: Uuid,
    ) -> Result<Option<TaskExecutionRecord>, RepositoryError>;

    async fn prepared_executions(
        &self,
        limit: usize,
    ) -> Result<Vec<TaskExecutionRecord>, RepositoryError>;

    async fn succeeded_unfinalized_executions(
        &self,
        limit: usize,
    ) -> Result<Vec<TaskExecutionRecord>, RepositoryError>;

    async fn mark_claim_finalized(
        &self,
        execution_id: Uuid,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<(), RepositoryError>;
}

#[async_trait]
pub trait TaskActionRunner: Send + Sync {
    async fn run(&self, execution: &TaskExecutionRecord) -> Result<Value, TaskActionRunError>;
}

#[derive(Clone)]
pub struct TaskExecutionService<R>
where
    R: TaskExecutionRepository,
{
    repository: Arc<R>,
}

impl<R> TaskExecutionService<R>
where
    R: TaskExecutionRepository,
{
    #[must_use]
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    pub async fn register_spec(
        &self,
        spec: &TaskExecutionSpec,
    ) -> Result<TaskExecutionSpec, TaskExecutionError> {
        spec.action.validate()?;
        self.repository
            .register_spec(spec)
            .await
            .map_err(TaskExecutionError::Repository)
    }

    pub async fn grant_tool_approval(
        &self,
        approval: &ToolExecutionApproval,
    ) -> Result<ToolExecutionApproval, TaskExecutionError> {
        self.repository
            .grant_tool_approval(approval)
            .await
            .map_err(TaskExecutionError::Repository)
    }

    pub async fn prepare_claim(
        &self,
        claim_id: Uuid,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<Option<TaskExecutionRecord>, TaskExecutionError> {
        self.repository
            .prepare_claim_execution(claim_id, now, correlation_id, actor_id)
            .await
            .map_err(TaskExecutionError::Repository)
    }

    pub async fn execute_prepared<A>(
        &self,
        execution_id: Uuid,
        runner: &A,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<TaskExecutionRecord, TaskExecutionError>
    where
        A: TaskActionRunner,
    {
        let running = self
            .repository
            .mark_execution_running(execution_id, now, correlation_id, actor_id)
            .await
            .map_err(TaskExecutionError::Repository)?;
        match runner.run(&running).await {
            Ok(output) => self
                .repository
                .succeed_execution(
                    execution_id,
                    output,
                    Utc::now(),
                    correlation_id,
                    actor_id,
                )
                .await
                .map_err(TaskExecutionError::Repository),
            Err(error) => self
                .repository
                .fail_execution(
                    execution_id,
                    &error.to_string(),
                    Utc::now(),
                    correlation_id,
                    actor_id,
                )
                .await
                .map_err(TaskExecutionError::Repository),
        }
    }
}

#[derive(Debug, Error)]
pub enum TaskActionRunError {
    #[error("task action is not supported: {0}")]
    Unsupported(String),
    #[error("task action failed: {0}")]
    Failed(String),
}

#[derive(Debug, Error)]
pub enum TaskExecutionError {
    #[error("task execution validation failed: {0}")]
    Validation(String),
    #[error("task execution repository failed: {0}")]
    Repository(RepositoryError),
}
