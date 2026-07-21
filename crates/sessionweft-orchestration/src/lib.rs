use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sessionweft_core::{EventEnvelope, SessionId};
use thiserror::Error;
use uuid::Uuid;

pub const WORKFLOW_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    pub name: String,
    pub version: u32,
    pub nodes: Vec<WorkflowNodeDefinition>,
}

impl WorkflowDefinition {
    pub fn validate(&self) -> Result<(), OrchestrationError> {
        if self.name.trim().is_empty() {
            return Err(OrchestrationError::Validation(
                "workflow name cannot be empty".into(),
            ));
        }
        if self.nodes.is_empty() {
            return Err(OrchestrationError::Validation(
                "workflow must contain at least one node".into(),
            ));
        }

        let mut ids = BTreeSet::new();
        for node in &self.nodes {
            node.validate()?;
            if !ids.insert(node.id.clone()) {
                return Err(OrchestrationError::Validation(format!(
                    "duplicate workflow node '{}'",
                    node.id
                )));
            }
        }

        for node in &self.nodes {
            for dependency in &node.dependencies {
                if dependency == &node.id {
                    return Err(OrchestrationError::Validation(format!(
                        "node '{}' cannot depend on itself",
                        node.id
                    )));
                }
                if !ids.contains(dependency) {
                    return Err(OrchestrationError::Validation(format!(
                        "node '{}' depends on missing node '{}'",
                        node.id, dependency
                    )));
                }
            }
            if let Some(fallback) = &node.fallback {
                if !ids.contains(fallback) {
                    return Err(OrchestrationError::Validation(format!(
                        "node '{}' references missing fallback '{}'",
                        node.id, fallback
                    )));
                }
            }
        }

        let mut indegree = self
            .nodes
            .iter()
            .map(|node| (node.id.clone(), node.dependencies.len()))
            .collect::<BTreeMap<_, _>>();
        let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for node in &self.nodes {
            for dependency in &node.dependencies {
                dependents
                    .entry(dependency.clone())
                    .or_default()
                    .push(node.id.clone());
            }
        }

        let mut queue = indegree
            .iter()
            .filter_map(|(id, count)| (*count == 0).then_some(id.clone()))
            .collect::<VecDeque<_>>();
        let mut visited = 0_usize;
        while let Some(node_id) = queue.pop_front() {
            visited += 1;
            if let Some(children) = dependents.get(&node_id) {
                for child in children {
                    let count = indegree.get_mut(child).ok_or_else(|| {
                        OrchestrationError::Validation(
                            "workflow graph became internally inconsistent".into(),
                        )
                    })?;
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        queue.push_back(child.clone());
                    }
                }
            }
        }

        if visited != self.nodes.len() {
            return Err(OrchestrationError::Validation(
                "workflow graph contains a cycle".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowNodeDefinition {
    pub id: String,
    #[serde(default)]
    pub kind: WorkflowNodeKind,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(default)]
    pub continue_on_failure: bool,
    #[serde(default)]
    pub fallback: Option<String>,
}

impl WorkflowNodeDefinition {
    fn validate(&self) -> Result<(), OrchestrationError> {
        if self.id.trim().is_empty() {
            return Err(OrchestrationError::Validation(
                "workflow node ID cannot be empty".into(),
            ));
        }
        if self.id.len() > 128 {
            return Err(OrchestrationError::Validation(format!(
                "workflow node '{}' exceeds 128 bytes",
                self.id
            )));
        }
        if self.max_attempts == 0 || self.max_attempts > 100 {
            return Err(OrchestrationError::Validation(format!(
                "workflow node '{}' max_attempts must be between 1 and 100",
                self.id
            )));
        }
        Ok(())
    }
}

const fn default_max_attempts() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeKind {
    #[default]
    Task,
    Approval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeStatus {
    Pending,
    Ready,
    Running,
    WaitingApproval,
    Succeeded,
    Failed,
    Skipped,
}

impl WorkflowNodeStatus {
    const fn satisfies_dependency(self, continue_on_failure: bool) -> bool {
        matches!(self, Self::Succeeded | Self::Skipped)
            || (continue_on_failure && matches!(self, Self::Failed))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowNodeExecution {
    pub status: WorkflowNodeStatus,
    pub attempts: u32,
    pub owner_id: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

impl Default for WorkflowNodeExecution {
    fn default() -> Self {
        Self {
            status: WorkflowNodeStatus::Pending,
            attempts: 0,
            owner_id: None,
            started_at: None,
            completed_at: None,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowExecution {
    pub schema_version: u32,
    pub id: Uuid,
    pub session_id: SessionId,
    pub version: u64,
    pub status: WorkflowStatus,
    pub definition: WorkflowDefinition,
    pub nodes: BTreeMap<String, WorkflowNodeExecution>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl WorkflowExecution {
    pub fn new(
        session_id: SessionId,
        definition: WorkflowDefinition,
    ) -> Result<Self, OrchestrationError> {
        definition.validate()?;
        let now = Utc::now();
        let nodes = definition
            .nodes
            .iter()
            .map(|node| (node.id.clone(), WorkflowNodeExecution::default()))
            .collect();
        let mut execution = Self {
            schema_version: WORKFLOW_SCHEMA_VERSION,
            id: Uuid::new_v4(),
            session_id,
            version: 0,
            status: WorkflowStatus::Running,
            definition,
            nodes,
            created_at: now,
            updated_at: now,
        };
        execution.refresh_ready_nodes()?;
        Ok(execution)
    }

    #[must_use]
    pub fn ready_nodes(&self) -> Vec<String> {
        self.nodes
            .iter()
            .filter_map(|(id, state)| {
                (state.status == WorkflowNodeStatus::Ready).then_some(id.clone())
            })
            .collect()
    }

    pub fn start_node(
        &mut self,
        expected_version: u64,
        node_id: &str,
        owner_id: impl Into<String>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<EventEnvelope, OrchestrationError> {
        self.ensure_running(expected_version)?;
        let state = self.node_state_mut(node_id)?;
        if state.status != WorkflowNodeStatus::Ready {
            return Err(OrchestrationError::InvalidTransition(format!(
                "node '{node_id}' is not ready"
            )));
        }
        state.status = WorkflowNodeStatus::Running;
        state.attempts = state.attempts.saturating_add(1);
        state.owner_id = Some(owner_id.into());
        state.started_at = Some(Utc::now());
        state.completed_at = None;
        state.last_error = None;
        self.advance_version();

        Ok(self.event(
            "workflow.node_started",
            node_id,
            correlation_id,
            actor_id,
            json!({"attempt": self.nodes[node_id].attempts}),
        ))
    }

    pub fn complete_node(
        &mut self,
        expected_version: u64,
        node_id: &str,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<Vec<EventEnvelope>, OrchestrationError> {
        self.ensure_running(expected_version)?;
        let state = self.node_state_mut(node_id)?;
        if state.status != WorkflowNodeStatus::Running {
            return Err(OrchestrationError::InvalidTransition(format!(
                "node '{node_id}' is not running"
            )));
        }
        state.status = WorkflowNodeStatus::Succeeded;
        state.completed_at = Some(Utc::now());
        state.owner_id = None;
        self.advance_version();
        self.refresh_ready_nodes()?;

        let mut events = vec![self.event(
            "workflow.node_completed",
            node_id,
            correlation_id,
            actor_id,
            json!({}),
        )];
        if self.is_successfully_complete() {
            self.status = WorkflowStatus::Succeeded;
            events.push(EventEnvelope::new(
                "workflow.completed",
                Some(self.session_id),
                correlation_id,
                actor_id,
                json!({"workflow_id": self.id, "workflow_version": self.version}),
            ));
        }
        Ok(events)
    }

    pub fn fail_node(
        &mut self,
        expected_version: u64,
        node_id: &str,
        sanitized_error: impl Into<String>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<Vec<EventEnvelope>, OrchestrationError> {
        self.ensure_running(expected_version)?;
        let definition = self.node_definition(node_id)?.clone();
        let error = sanitized_error.into();
        let state = self.node_state_mut(node_id)?;
        if state.status != WorkflowNodeStatus::Running {
            return Err(OrchestrationError::InvalidTransition(format!(
                "node '{node_id}' is not running"
            )));
        }
        state.last_error = Some(error.clone());
        state.owner_id = None;
        state.completed_at = Some(Utc::now());

        let retry = state.attempts < definition.max_attempts;
        state.status = if retry {
            WorkflowNodeStatus::Ready
        } else {
            WorkflowNodeStatus::Failed
        };
        self.advance_version();

        let mut events = vec![self.event(
            if retry {
                "workflow.node_retry_scheduled"
            } else {
                "workflow.node_failed"
            },
            node_id,
            correlation_id,
            actor_id,
            json!({
                "attempt": self.nodes[node_id].attempts,
                "max_attempts": definition.max_attempts,
                "error": error,
            }),
        )];

        if !retry {
            if let Some(fallback) = definition.fallback {
                let fallback_state = self.node_state_mut(&fallback)?;
                if fallback_state.status == WorkflowNodeStatus::Pending {
                    fallback_state.status = WorkflowNodeStatus::Ready;
                }
                events.push(self.event(
                    "workflow.fallback_activated",
                    &fallback,
                    correlation_id,
                    actor_id,
                    json!({"failed_node": node_id}),
                ));
            }
            self.refresh_ready_nodes()?;
            if !definition.continue_on_failure && !self.has_recoverable_work() {
                self.status = WorkflowStatus::Failed;
                events.push(EventEnvelope::new(
                    "workflow.failed",
                    Some(self.session_id),
                    correlation_id,
                    actor_id,
                    json!({
                        "workflow_id": self.id,
                        "workflow_version": self.version,
                        "failed_node": node_id,
                    }),
                ));
            }
        }
        Ok(events)
    }

    pub fn decide_approval(
        &mut self,
        expected_version: u64,
        node_id: &str,
        approved: bool,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<Vec<EventEnvelope>, OrchestrationError> {
        self.ensure_running(expected_version)?;
        let definition = self.node_definition(node_id)?;
        if definition.kind != WorkflowNodeKind::Approval {
            return Err(OrchestrationError::Validation(format!(
                "node '{node_id}' is not an approval node"
            )));
        }
        let state = self.node_state_mut(node_id)?;
        if state.status != WorkflowNodeStatus::WaitingApproval {
            return Err(OrchestrationError::InvalidTransition(format!(
                "node '{node_id}' is not waiting for approval"
            )));
        }
        state.status = if approved {
            WorkflowNodeStatus::Succeeded
        } else {
            WorkflowNodeStatus::Failed
        };
        state.completed_at = Some(Utc::now());
        self.advance_version();
        self.refresh_ready_nodes()?;

        let mut events = vec![self.event(
            if approved {
                "workflow.approval_granted"
            } else {
                "workflow.approval_rejected"
            },
            node_id,
            correlation_id,
            actor_id,
            json!({"approved": approved}),
        )];
        if approved && self.is_successfully_complete() {
            self.status = WorkflowStatus::Succeeded;
            events.push(EventEnvelope::new(
                "workflow.completed",
                Some(self.session_id),
                correlation_id,
                actor_id,
                json!({"workflow_id": self.id, "workflow_version": self.version}),
            ));
        } else if !approved {
            self.status = WorkflowStatus::Failed;
            events.push(EventEnvelope::new(
                "workflow.failed",
                Some(self.session_id),
                correlation_id,
                actor_id,
                json!({"workflow_id": self.id, "rejected_node": node_id}),
            ));
        }
        Ok(events)
    }

    fn refresh_ready_nodes(&mut self) -> Result<(), OrchestrationError> {
        let definitions = self.definition.nodes.clone();
        for definition in definitions {
            let current = self.node_state(&definition.id)?.status;
            if current != WorkflowNodeStatus::Pending {
                continue;
            }
            let dependencies_satisfied = definition.dependencies.iter().all(|dependency| {
                let dependency_definition = self.node_definition(dependency);
                let dependency_state = self.node_state(dependency);
                match (dependency_definition, dependency_state) {
                    (Ok(definition), Ok(state)) => state
                        .status
                        .satisfies_dependency(definition.continue_on_failure),
                    _ => false,
                }
            });
            if dependencies_satisfied {
                self.node_state_mut(&definition.id)?.status =
                    if definition.kind == WorkflowNodeKind::Approval {
                        WorkflowNodeStatus::WaitingApproval
                    } else {
                        WorkflowNodeStatus::Ready
                    };
            }
        }
        Ok(())
    }

    fn is_successfully_complete(&self) -> bool {
        self.definition.nodes.iter().all(|definition| {
            self.nodes.get(&definition.id).is_some_and(|state| {
                state
                    .status
                    .satisfies_dependency(definition.continue_on_failure)
            })
        })
    }

    fn has_recoverable_work(&self) -> bool {
        self.nodes.values().any(|state| {
            matches!(
                state.status,
                WorkflowNodeStatus::Ready
                    | WorkflowNodeStatus::Running
                    | WorkflowNodeStatus::WaitingApproval
            )
        })
    }

    fn ensure_running(&self, expected_version: u64) -> Result<(), OrchestrationError> {
        if self.version != expected_version {
            return Err(OrchestrationError::VersionConflict {
                expected: expected_version,
                actual: self.version,
            });
        }
        if self.status != WorkflowStatus::Running {
            return Err(OrchestrationError::InvalidTransition(
                "workflow is not running".into(),
            ));
        }
        Ok(())
    }

    fn node_definition(
        &self,
        node_id: &str,
    ) -> Result<&WorkflowNodeDefinition, OrchestrationError> {
        self.definition
            .nodes
            .iter()
            .find(|node| node.id == node_id)
            .ok_or_else(|| OrchestrationError::NodeNotFound(node_id.into()))
    }

    fn node_state(&self, node_id: &str) -> Result<&WorkflowNodeExecution, OrchestrationError> {
        self.nodes
            .get(node_id)
            .ok_or_else(|| OrchestrationError::NodeNotFound(node_id.into()))
    }

    fn node_state_mut(
        &mut self,
        node_id: &str,
    ) -> Result<&mut WorkflowNodeExecution, OrchestrationError> {
        self.nodes
            .get_mut(node_id)
            .ok_or_else(|| OrchestrationError::NodeNotFound(node_id.into()))
    }

    fn advance_version(&mut self) {
        self.version = self.version.saturating_add(1);
        self.updated_at = Utc::now();
    }

    fn event(
        &self,
        event_type: &str,
        node_id: &str,
        correlation_id: Uuid,
        actor_id: Option<&str>,
        details: serde_json::Value,
    ) -> EventEnvelope {
        EventEnvelope::new(
            event_type,
            Some(self.session_id),
            correlation_id,
            actor_id,
            json!({
                "workflow_id": self.id,
                "workflow_version": self.version,
                "node_id": node_id,
                "details": details,
            }),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LockResource {
    Workspace {
        workspace_id: String,
    },
    Directory {
        workspace_id: String,
        path: String,
    },
    File {
        workspace_id: String,
        path: String,
    },
    Symbol {
        workspace_id: String,
        path: String,
        symbol: String,
    },
}

impl LockResource {
    pub fn validate(&self) -> Result<(), OrchestrationError> {
        let workspace_id = self.workspace_id();
        if workspace_id.trim().is_empty() {
            return Err(OrchestrationError::Validation(
                "workspace ID cannot be empty".into(),
            ));
        }
        match self {
            Self::Workspace { .. } => {}
            Self::Directory { path, .. } | Self::File { path, .. } => {
                validate_relative_path(path)?;
            }
            Self::Symbol { path, symbol, .. } => {
                validate_relative_path(path)?;
                if symbol.trim().is_empty() {
                    return Err(OrchestrationError::Validation(
                        "symbol cannot be empty".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn workspace_id(&self) -> &str {
        match self {
            Self::Workspace { workspace_id }
            | Self::Directory { workspace_id, .. }
            | Self::File { workspace_id, .. }
            | Self::Symbol { workspace_id, .. } => workspace_id,
        }
    }

    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        if self.workspace_id() != other.workspace_id() {
            return false;
        }
        let left = self.segments();
        let right = other.segments();
        is_prefix(&left, &right) || is_prefix(&right, &left)
    }

    fn segments(&self) -> Vec<String> {
        let mut segments = vec![format!("workspace:{}", self.workspace_id())];
        match self {
            Self::Workspace { .. } => {}
            Self::Directory { path, .. } => {
                segments.extend(path_segments(path));
            }
            Self::File { path, .. } => {
                segments.extend(path_segments(path));
                segments.push("$file".into());
            }
            Self::Symbol { path, symbol, .. } => {
                segments.extend(path_segments(path));
                segments.push("$file".into());
                segments.push(format!("$symbol:{symbol}"));
            }
        }
        segments
    }
}

fn validate_relative_path(path: &str) -> Result<(), OrchestrationError> {
    if path.trim().is_empty() {
        return Err(OrchestrationError::Validation(
            "resource path cannot be empty".into(),
        ));
    }
    if path.starts_with('/')
        || path.starts_with('\\')
        || path.split(['/', '\\']).any(|segment| segment == "..")
    {
        return Err(OrchestrationError::Validation(
            "resource path must remain inside the workspace".into(),
        ));
    }
    Ok(())
}

fn path_segments(path: &str) -> Vec<String> {
    path.split(['/', '\\'])
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .map(ToOwned::to_owned)
        .collect()
}

fn is_prefix(left: &[String], right: &[String]) -> bool {
    left.len() <= right.len() && left.iter().zip(right).all(|(left, right)| left == right)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockMode {
    Shared,
    Exclusive,
}

impl LockMode {
    #[must_use]
    pub const fn conflicts_with(self, other: Self) -> bool {
        matches!(self, Self::Exclusive) || matches!(other, Self::Exclusive)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockRequest {
    pub session_id: SessionId,
    pub owner_id: String,
    pub resource: LockResource,
    pub mode: LockMode,
    pub ttl_seconds: u32,
}

impl LockRequest {
    pub fn validate(&self) -> Result<(), OrchestrationError> {
        if self.owner_id.trim().is_empty() {
            return Err(OrchestrationError::Validation(
                "lock owner cannot be empty".into(),
            ));
        }
        if !(1..=3_600).contains(&self.ttl_seconds) {
            return Err(OrchestrationError::Validation(
                "lock TTL must be between 1 and 3600 seconds".into(),
            ));
        }
        self.resource.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockLease {
    pub lock_id: Uuid,
    pub session_id: SessionId,
    pub owner_id: String,
    pub resource: LockResource,
    pub mode: LockMode,
    pub fencing_token: u64,
    pub acquired_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl LockLease {
    #[must_use]
    pub fn conflicts_with(&self, request: &LockRequest, now: DateTime<Utc>) -> bool {
        self.expires_at > now
            && self.resource.overlaps(&request.resource)
            && self.mode.conflicts_with(request.mode)
    }
}

#[async_trait]
pub trait OrchestrationRepository: Send + Sync {
    async fn create_workflow(
        &self,
        execution: &WorkflowExecution,
        events: &[EventEnvelope],
    ) -> Result<WorkflowExecution, RepositoryError>;

    async fn get_workflow(
        &self,
        workflow_id: Uuid,
    ) -> Result<Option<WorkflowExecution>, RepositoryError>;

    async fn save_workflow(
        &self,
        expected_version: u64,
        execution: &WorkflowExecution,
        events: &[EventEnvelope],
    ) -> Result<WorkflowExecution, RepositoryError>;

    async fn acquire_lock(
        &self,
        request: &LockRequest,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<LockLease, RepositoryError>;

    async fn heartbeat_lock(
        &self,
        lock_id: Uuid,
        owner_id: &str,
        fencing_token: u64,
        ttl_seconds: u32,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<LockLease, RepositoryError>;

    async fn release_lock(
        &self,
        lock_id: Uuid,
        owner_id: &str,
        fencing_token: u64,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<(), RepositoryError>;

    async fn validate_fence(
        &self,
        resource: &LockResource,
        owner_id: &str,
        fencing_token: u64,
        now: DateTime<Utc>,
    ) -> Result<(), RepositoryError>;

    async fn list_locks(
        &self,
        workspace_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<LockLease>, RepositoryError>;
}

#[derive(Clone)]
pub struct OrchestrationService<R>
where
    R: OrchestrationRepository,
{
    repository: Arc<R>,
}

impl<R> OrchestrationService<R>
where
    R: OrchestrationRepository,
{
    #[must_use]
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    pub async fn create_workflow(
        &self,
        session_id: SessionId,
        definition: WorkflowDefinition,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<WorkflowExecution, OrchestrationError> {
        let execution = WorkflowExecution::new(session_id, definition)?;
        let event = EventEnvelope::new(
            "workflow.created",
            Some(session_id),
            correlation_id,
            actor_id,
            json!({
                "workflow_id": execution.id,
                "workflow_version": execution.version,
                "ready_nodes": execution.ready_nodes(),
            }),
        );
        self.repository
            .create_workflow(&execution, &[event])
            .await
            .map_err(OrchestrationError::Repository)
    }

    pub async fn get_workflow(
        &self,
        workflow_id: Uuid,
    ) -> Result<WorkflowExecution, OrchestrationError> {
        self.repository
            .get_workflow(workflow_id)
            .await
            .map_err(OrchestrationError::Repository)?
            .ok_or(OrchestrationError::WorkflowNotFound(workflow_id))
    }

    pub async fn start_node(
        &self,
        workflow_id: Uuid,
        expected_version: u64,
        node_id: &str,
        owner_id: impl Into<String>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<WorkflowExecution, OrchestrationError> {
        let mut execution = self.get_workflow(workflow_id).await?;
        let event = execution.start_node(
            expected_version,
            node_id,
            owner_id,
            correlation_id,
            actor_id,
        )?;
        self.repository
            .save_workflow(expected_version, &execution, &[event])
            .await
            .map_err(OrchestrationError::Repository)
    }

    pub async fn complete_node(
        &self,
        workflow_id: Uuid,
        expected_version: u64,
        node_id: &str,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<WorkflowExecution, OrchestrationError> {
        let mut execution = self.get_workflow(workflow_id).await?;
        let events =
            execution.complete_node(expected_version, node_id, correlation_id, actor_id)?;
        self.repository
            .save_workflow(expected_version, &execution, &events)
            .await
            .map_err(OrchestrationError::Repository)
    }

    pub async fn fail_node(
        &self,
        workflow_id: Uuid,
        expected_version: u64,
        node_id: &str,
        sanitized_error: impl Into<String>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<WorkflowExecution, OrchestrationError> {
        let mut execution = self.get_workflow(workflow_id).await?;
        let events = execution.fail_node(
            expected_version,
            node_id,
            sanitized_error,
            correlation_id,
            actor_id,
        )?;
        self.repository
            .save_workflow(expected_version, &execution, &events)
            .await
            .map_err(OrchestrationError::Repository)
    }

    pub async fn decide_approval(
        &self,
        workflow_id: Uuid,
        expected_version: u64,
        node_id: &str,
        approved: bool,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<WorkflowExecution, OrchestrationError> {
        let mut execution = self.get_workflow(workflow_id).await?;
        let events = execution.decide_approval(
            expected_version,
            node_id,
            approved,
            correlation_id,
            actor_id,
        )?;
        self.repository
            .save_workflow(expected_version, &execution, &events)
            .await
            .map_err(OrchestrationError::Repository)
    }

    pub async fn acquire_lock(
        &self,
        request: &LockRequest,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<LockLease, OrchestrationError> {
        request.validate()?;
        self.repository
            .acquire_lock(request, correlation_id, actor_id)
            .await
            .map_err(OrchestrationError::Repository)
    }

    pub async fn heartbeat_lock(
        &self,
        lock_id: Uuid,
        owner_id: &str,
        fencing_token: u64,
        ttl_seconds: u32,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<LockLease, OrchestrationError> {
        if !(1..=3_600).contains(&ttl_seconds) {
            return Err(OrchestrationError::Validation(
                "lock TTL must be between 1 and 3600 seconds".into(),
            ));
        }
        self.repository
            .heartbeat_lock(
                lock_id,
                owner_id,
                fencing_token,
                ttl_seconds,
                correlation_id,
                actor_id,
            )
            .await
            .map_err(OrchestrationError::Repository)
    }

    pub async fn release_lock(
        &self,
        lock_id: Uuid,
        owner_id: &str,
        fencing_token: u64,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<(), OrchestrationError> {
        self.repository
            .release_lock(lock_id, owner_id, fencing_token, correlation_id, actor_id)
            .await
            .map_err(OrchestrationError::Repository)
    }

    pub async fn validate_fence(
        &self,
        resource: &LockResource,
        owner_id: &str,
        fencing_token: u64,
    ) -> Result<(), OrchestrationError> {
        resource.validate()?;
        self.repository
            .validate_fence(resource, owner_id, fencing_token, Utc::now())
            .await
            .map_err(OrchestrationError::Repository)
    }

    pub async fn list_locks(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<LockLease>, OrchestrationError> {
        self.repository
            .list_locks(workspace_id, Utc::now())
            .await
            .map_err(OrchestrationError::Repository)
    }
}

#[derive(Debug, Error)]
pub enum OrchestrationError {
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("workflow {0} not found")]
    WorkflowNotFound(Uuid),
    #[error("workflow node '{0}' not found")]
    NodeNotFound(String),
    #[error("workflow version conflict: expected {expected}, actual {actual}")]
    VersionConflict { expected: u64, actual: u64 },
    #[error("invalid orchestration transition: {0}")]
    InvalidTransition(String),
    #[error("orchestration repository error: {0}")]
    Repository(RepositoryError),
}

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("backend error: {0}")]
    Backend(String),
    #[error("workflow {0} not found")]
    WorkflowNotFound(Uuid),
    #[error("workflow version conflict: expected {expected}, actual {actual}")]
    VersionConflict { expected: u64, actual: u64 },
    #[error("lock conflict with owner '{owner_id}' on {resource:?}")]
    LockConflict {
        owner_id: String,
        resource: LockResource,
    },
    #[error("lock {0} not found")]
    LockNotFound(Uuid),
    #[error("stale or invalid fencing token")]
    StaleFence,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, dependencies: &[&str]) -> WorkflowNodeDefinition {
        WorkflowNodeDefinition {
            id: id.into(),
            kind: WorkflowNodeKind::Task,
            dependencies: dependencies.iter().map(|value| (*value).into()).collect(),
            max_attempts: 1,
            continue_on_failure: false,
            fallback: None,
        }
    }

    #[test]
    fn rejects_workflow_cycles() {
        let definition = WorkflowDefinition {
            name: "cycle".into(),
            version: 1,
            nodes: vec![node("a", &["b"]), node("b", &["a"])],
        };
        assert!(definition.validate().is_err());
    }

    #[test]
    fn fan_out_nodes_become_ready_after_dependency_completion() {
        let definition = WorkflowDefinition {
            name: "fan-out".into(),
            version: 1,
            nodes: vec![
                node("plan", &[]),
                node("worker-a", &["plan"]),
                node("worker-b", &["plan"]),
            ],
        };
        let mut execution = WorkflowExecution::new(SessionId::new(), definition).expect("workflow");
        assert_eq!(execution.ready_nodes(), vec!["plan"]);
        execution
            .start_node(0, "plan", "planner", Uuid::new_v4(), None)
            .expect("start");
        execution
            .complete_node(1, "plan", Uuid::new_v4(), None)
            .expect("complete");
        assert_eq!(execution.ready_nodes(), vec!["worker-a", "worker-b"]);
    }

    #[test]
    fn parent_and_child_resources_overlap() {
        let directory = LockResource::Directory {
            workspace_id: "ws".into(),
            path: "src".into(),
        };
        let file = LockResource::File {
            workspace_id: "ws".into(),
            path: "src/lib.rs".into(),
        };
        let sibling = LockResource::File {
            workspace_id: "ws".into(),
            path: "src2/lib.rs".into(),
        };
        assert!(directory.overlaps(&file));
        assert!(!directory.overlaps(&sibling));
    }

    #[test]
    fn exclusive_lock_conflicts_with_shared_child() {
        let now = Utc::now();
        let lease = LockLease {
            lock_id: Uuid::new_v4(),
            session_id: SessionId::new(),
            owner_id: "one".into(),
            resource: LockResource::Directory {
                workspace_id: "ws".into(),
                path: "src".into(),
            },
            mode: LockMode::Exclusive,
            fencing_token: 1,
            acquired_at: now,
            expires_at: now + Duration::seconds(30),
        };
        let request = LockRequest {
            session_id: SessionId::new(),
            owner_id: "two".into(),
            resource: LockResource::File {
                workspace_id: "ws".into(),
                path: "src/lib.rs".into(),
            },
            mode: LockMode::Shared,
            ttl_seconds: 30,
        };
        assert!(lease.conflicts_with(&request, now));
    }
}
