use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sessionweft_core::SessionId;
use sessionweft_orchestration::{LockMode, LockResource, WorkflowExecution, WorkflowNodeKind};
use uuid::Uuid;

use super::{ClaimRequest, ClaimState, HandoverRequest, RepositoryError, SchedulerError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredLock {
    pub resource: LockResource,
    pub mode: LockMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskLockRequirement {
    pub workflow_id: Uuid,
    pub session_id: SessionId,
    pub node_id: String,
    pub required: RequiredLock,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TaskLockRequirement {
    pub fn new(
        workflow: &WorkflowExecution,
        node_id: impl Into<String>,
        required: RequiredLock,
    ) -> Result<Self, SchedulerError> {
        let node_id = node_id.into();
        let is_task = workflow
            .definition
            .nodes
            .iter()
            .any(|node| node.id == node_id && node.kind == WorkflowNodeKind::Task);
        if !is_task {
            return Err(SchedulerError::Validation(format!(
                "lock prerequisite references non-task or missing node '{node_id}'"
            )));
        }
        required.resource.validate().map_err(|error| {
            SchedulerError::Validation(format!("invalid lock prerequisite: {error}"))
        })?;
        let now = Utc::now();
        Ok(Self {
            workflow_id: workflow.id,
            session_id: workflow.session_id,
            node_id,
            required,
            created_at: now,
            updated_at: now,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimLockFence {
    pub lock_id: Uuid,
    pub resource: LockResource,
    pub mode: LockMode,
    pub fencing_token: u64,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimLockFenceSnapshot {
    pub claim_id: Uuid,
    pub workflow_id: Uuid,
    pub node_id: String,
    pub agent_id: Uuid,
    pub fence: ClaimLockFence,
    pub created_at: DateTime<Utc>,
}

#[async_trait]
pub trait SchedulerPrerequisiteRepository: Send + Sync {
    async fn register_lock_requirement(
        &self,
        requirement: &TaskLockRequirement,
    ) -> Result<TaskLockRequirement, RepositoryError>;

    async fn get_lock_requirement(
        &self,
        workflow_id: Uuid,
        node_id: &str,
    ) -> Result<Option<TaskLockRequirement>, RepositoryError>;

    async fn get_claim_lock_fence(
        &self,
        claim_id: Uuid,
    ) -> Result<Option<ClaimLockFenceSnapshot>, RepositoryError>;

    async fn claim_next_guarded(
        &self,
        request: &ClaimRequest,
        now: DateTime<Utc>,
    ) -> Result<Option<ClaimState>, RepositoryError>;

    async fn handover_released_claim_guarded(
        &self,
        request: &HandoverRequest,
    ) -> Result<Option<ClaimState>, RepositoryError>;
}

#[derive(Clone)]
pub struct SchedulerPrerequisiteService<R>
where
    R: SchedulerPrerequisiteRepository,
{
    repository: Arc<R>,
}

impl<R> SchedulerPrerequisiteService<R>
where
    R: SchedulerPrerequisiteRepository,
{
    #[must_use]
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    pub async fn require_lock(
        &self,
        workflow: &WorkflowExecution,
        node_id: impl Into<String>,
        required: RequiredLock,
    ) -> Result<TaskLockRequirement, SchedulerError> {
        let requirement = TaskLockRequirement::new(workflow, node_id, required)?;
        self.repository
            .register_lock_requirement(&requirement)
            .await
            .map_err(SchedulerError::Repository)
    }

    pub async fn claim_lock_fence(
        &self,
        claim_id: Uuid,
    ) -> Result<Option<ClaimLockFenceSnapshot>, SchedulerError> {
        self.repository
            .get_claim_lock_fence(claim_id)
            .await
            .map_err(SchedulerError::Repository)
    }
}
