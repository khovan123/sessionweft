use std::{collections::{BTreeMap, BTreeSet}, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sessionweft_core::SessionId;
use sessionweft_execution::{AgentRecord, AgentRole, Capability};
use sessionweft_orchestration::{WorkflowExecution, WorkflowNodeKind};
use thiserror::Error;
use uuid::Uuid;

pub const SCHEDULER_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRequirement {
    #[serde(default)]
    pub role: Option<AgentRole>,
    #[serde(default)]
    pub capabilities: BTreeSet<Capability>,
}

impl TaskRequirement {
    pub fn validate(&self) -> Result<(), SchedulerError> {
        for capability in &self.capabilities {
            if let Capability::Tool(name) = capability
                && name.trim().is_empty()
            {
                return Err(SchedulerError::Validation(
                    "tool capability name cannot be empty".into(),
                ));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn matches(&self, agent: &AgentRecord) -> bool {
        self.role.is_none_or(|role| role == agent.manifest.role)
            && self
                .capabilities
                .iter()
                .all(|capability| agent.manifest.capabilities.contains(capability))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerPlan {
    pub schema_version: u32,
    pub workflow_id: Uuid,
    pub session_id: SessionId,
    pub requirements: BTreeMap<String, TaskRequirement>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SchedulerPlan {
    pub fn new(
        workflow: &WorkflowExecution,
        requirements: BTreeMap<String, TaskRequirement>,
    ) -> Result<Self, SchedulerError> {
        let task_nodes = workflow
            .definition
            .nodes
            .iter()
            .filter(|node| node.kind == WorkflowNodeKind::Task)
            .map(|node| node.id.as_str())
            .collect::<BTreeSet<_>>();

        for (node_id, requirement) in &requirements {
            if !task_nodes.contains(node_id.as_str()) {
                return Err(SchedulerError::Validation(format!(
                    "scheduler requirement references non-task or missing node '{node_id}'"
                )));
            }
            requirement.validate()?;
        }

        let now = Utc::now();
        Ok(Self {
            schema_version: SCHEDULER_SCHEMA_VERSION,
            workflow_id: workflow.id,
            session_id: workflow.session_id,
            requirements,
            created_at: now,
            updated_at: now,
        })
    }

    #[must_use]
    pub fn requirement_for(&self, node_id: &str) -> TaskRequirement {
        self.requirements.get(node_id).cloned().unwrap_or(TaskRequirement {
            role: None,
            capabilities: BTreeSet::new(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskClaimStatus {
    Active,
    Completed,
    Failed,
    Released,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskClaim {
    pub schema_version: u32,
    pub id: Uuid,
    pub session_id: SessionId,
    pub workflow_id: Uuid,
    pub node_id: String,
    pub attempt: u32,
    pub agent_id: Uuid,
    pub task_id: String,
    pub idempotency_key: String,
    pub status: TaskClaimStatus,
    pub workflow_version: u64,
    pub agent_version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TaskClaim {
    #[must_use]
    pub fn new(
        workflow: &WorkflowExecution,
        node_id: impl Into<String>,
        attempt: u32,
        agent: &AgentRecord,
    ) -> Self {
        let node_id = node_id.into();
        let task_id = format!("{}:{node_id}:{attempt}", workflow.id);
        let now = Utc::now();
        Self {
            schema_version: SCHEDULER_SCHEMA_VERSION,
            id: Uuid::new_v4(),
            session_id: workflow.session_id,
            workflow_id: workflow.id,
            node_id,
            attempt,
            agent_id: agent.id,
            idempotency_key: task_id.clone(),
            task_id,
            status: TaskClaimStatus::Active,
            workflow_version: workflow.version,
            agent_version: agent.version,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn complete(&mut self, workflow_version: u64, agent_version: u64) {
        self.status = TaskClaimStatus::Completed;
        self.workflow_version = workflow_version;
        self.agent_version = agent_version;
        self.updated_at = Utc::now();
    }

    pub fn fail(&mut self, workflow_version: u64, agent_version: u64) {
        self.status = TaskClaimStatus::Failed;
        self.workflow_version = workflow_version;
        self.agent_version = agent_version;
        self.updated_at = Utc::now();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimRequest {
    pub workflow_id: Uuid,
    pub agent_id: Uuid,
    pub correlation_id: Uuid,
    pub actor_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimState {
    pub claim: TaskClaim,
    pub workflow: WorkflowExecution,
    pub agent: AgentRecord,
}

#[async_trait]
pub trait SchedulerRepository: Send + Sync {
    async fn register_plan(&self, plan: &SchedulerPlan) -> Result<SchedulerPlan, RepositoryError>;
    async fn get_plan(&self, workflow_id: Uuid) -> Result<Option<SchedulerPlan>, RepositoryError>;
    async fn claim_next(&self, request: &ClaimRequest) -> Result<Option<ClaimState>, RepositoryError>;
    async fn complete_claim(
        &self,
        claim_id: Uuid,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<ClaimState, RepositoryError>;
    async fn fail_claim(
        &self,
        claim_id: Uuid,
        sanitized_error: &str,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<ClaimState, RepositoryError>;
    async fn get_claim(&self, claim_id: Uuid) -> Result<Option<TaskClaim>, RepositoryError>;
}

#[derive(Clone)]
pub struct SchedulerService<R>
where
    R: SchedulerRepository,
{
    repository: Arc<R>,
}

impl<R> SchedulerService<R>
where
    R: SchedulerRepository,
{
    #[must_use]
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    pub async fn register_plan(
        &self,
        plan: &SchedulerPlan,
    ) -> Result<SchedulerPlan, SchedulerError> {
        self.repository
            .register_plan(plan)
            .await
            .map_err(SchedulerError::Repository)
    }

    pub async fn claim_next(
        &self,
        request: &ClaimRequest,
    ) -> Result<Option<ClaimState>, SchedulerError> {
        self.repository
            .claim_next(request)
            .await
            .map_err(SchedulerError::Repository)
    }

    pub async fn complete_claim(
        &self,
        claim_id: Uuid,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<ClaimState, SchedulerError> {
        self.repository
            .complete_claim(claim_id, correlation_id, actor_id)
            .await
            .map_err(SchedulerError::Repository)
    }

    pub async fn fail_claim(
        &self,
        claim_id: Uuid,
        sanitized_error: &str,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<ClaimState, SchedulerError> {
        if sanitized_error.trim().is_empty() {
            return Err(SchedulerError::Validation(
                "claim failure must include a sanitized error".into(),
            ));
        }
        self.repository
            .fail_claim(claim_id, sanitized_error, correlation_id, actor_id)
            .await
            .map_err(SchedulerError::Repository)
    }
}

#[derive(Debug, Error)]
pub enum SchedulerError {
    #[error("scheduler validation failed: {0}")]
    Validation(String),
    #[error("scheduler repository failed: {0}")]
    Repository(RepositoryError),
}

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("scheduler plan for workflow {0} not found")]
    PlanNotFound(Uuid),
    #[error("scheduler claim {0} not found")]
    ClaimNotFound(Uuid),
    #[error("scheduler claim {0} is not active")]
    ClaimNotActive(Uuid),
    #[error("workflow {0} not found")]
    WorkflowNotFound(Uuid),
    #[error("agent {0} not found")]
    AgentNotFound(Uuid),
    #[error("workflow and agent belong to different Sessions")]
    SessionMismatch,
    #[error("agent is not available for task assignment")]
    AgentUnavailable,
    #[error("scheduler version conflict: {0}")]
    Conflict(String),
    #[error("scheduler backend error: {0}")]
    Backend(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use sessionweft_execution::{AgentManifest, AgentStatus};
    use sessionweft_orchestration::{
        WorkflowDefinition, WorkflowNodeDefinition, WorkflowNodeStatus,
    };

    fn workflow() -> WorkflowExecution {
        WorkflowExecution::new(
            SessionId::new(),
            WorkflowDefinition {
                name: "scheduler".into(),
                version: 1,
                nodes: vec![WorkflowNodeDefinition {
                    id: "worker".into(),
                    kind: WorkflowNodeKind::Task,
                    dependencies: Vec::new(),
                    max_attempts: 2,
                    continue_on_failure: false,
                    fallback: None,
                }],
            },
        )
        .expect("workflow")
    }

    #[test]
    fn plan_rejects_missing_nodes() {
        let workflow = workflow();
        let result = SchedulerPlan::new(
            &workflow,
            BTreeMap::from([(
                "missing".into(),
                TaskRequirement {
                    role: None,
                    capabilities: BTreeSet::new(),
                },
            )]),
        );
        assert!(matches!(result, Err(SchedulerError::Validation(_))));
    }

    #[test]
    fn requirements_match_role_and_capabilities() {
        let workflow = workflow();
        assert_eq!(
            workflow.nodes["worker"].status,
            WorkflowNodeStatus::Ready
        );
        let agent = AgentRecord {
            schema_version: 1,
            id: Uuid::new_v4(),
            session_id: workflow.session_id,
            version: 0,
            manifest: AgentManifest {
                name: "worker".into(),
                role: AgentRole::Worker,
                capabilities: BTreeSet::from([Capability::WorkspaceWrite]),
                heartbeat_timeout_seconds: 30,
            },
            status: AgentStatus::Running,
            heartbeat_at: Utc::now(),
            current_task_id: None,
            last_error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let requirement = TaskRequirement {
            role: Some(AgentRole::Worker),
            capabilities: BTreeSet::from([Capability::WorkspaceWrite]),
        };
        assert!(requirement.matches(&agent));
    }
}
