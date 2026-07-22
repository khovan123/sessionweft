use std::collections::BTreeSet;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;
use sessionweft_control_plane::{ControlPlaneError, OperationContext};
use sessionweft_core::SessionId;
use sessionweft_execution::{AgentManifest, AgentRecord};
use sessionweft_knowledge::{MemoryClass, MemoryQuery, MemoryRecord, MemorySource};
use sessionweft_orchestration::{
    LockLease, LockMode, LockRequest, LockResource, WorkflowDefinition, WorkflowExecution,
};
use uuid::Uuid;

use super::{ApiError, AppState};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/sessions/{session_id}/agents",
            post(register_agent),
        )
        .route(
            "/v1/sessions/{session_id}/agents/{agent_id}",
            get(get_agent),
        )
        .route(
            "/v1/sessions/{session_id}/agents/{agent_id}/start",
            post(start_agent),
        )
        .route(
            "/v1/sessions/{session_id}/agents/{agent_id}/heartbeat",
            post(heartbeat_agent),
        )
        .route(
            "/v1/sessions/{session_id}/agents/{agent_id}/stop",
            post(stop_agent),
        )
        .route(
            "/v1/sessions/{session_id}/workflows",
            post(create_workflow),
        )
        .route(
            "/v1/sessions/{session_id}/workflows/{workflow_id}",
            get(get_workflow),
        )
        .route(
            "/v1/sessions/{session_id}/workflows/{workflow_id}/nodes/{node_id}/start",
            post(start_workflow_node),
        )
        .route(
            "/v1/sessions/{session_id}/workflows/{workflow_id}/nodes/{node_id}/complete",
            post(complete_workflow_node),
        )
        .route(
            "/v1/sessions/{session_id}/workflows/{workflow_id}/nodes/{node_id}/fail",
            post(fail_workflow_node),
        )
        .route(
            "/v1/sessions/{session_id}/workflows/{workflow_id}/nodes/{node_id}/approval",
            post(decide_workflow_approval),
        )
        .route(
            "/v1/sessions/{session_id}/locks",
            get(list_locks).post(acquire_lock),
        )
        .route(
            "/v1/sessions/{session_id}/locks/{lock_id}/heartbeat",
            post(heartbeat_lock),
        )
        .route(
            "/v1/sessions/{session_id}/locks/{lock_id}/release",
            post(release_lock),
        )
        .route(
            "/v1/sessions/{session_id}/memories",
            post(remember),
        )
        .route(
            "/v1/sessions/{session_id}/memories/search",
            post(search_memories),
        )
        .route(
            "/v1/sessions/{session_id}/memories/{memory_id}/forget",
            post(forget_memory),
        )
}

fn context() -> OperationContext {
    OperationContext::new(Uuid::new_v4(), Some("api".into()))
}

fn parse_session(value: &str, correlation_id: Uuid) -> Result<SessionId, ApiError> {
    value.parse().map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_session_id",
            "session ID must be a UUID",
            correlation_id,
            None,
        )
    })
}

fn parse_uuid(value: &str, name: &'static str, correlation_id: Uuid) -> Result<Uuid, ApiError> {
    Uuid::parse_str(value).map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_resource_id",
            format!("{name} must be a UUID"),
            correlation_id,
            None,
        )
    })
}

fn map_error(error: ControlPlaneError, correlation_id: Uuid) -> ApiError {
    match error {
        ControlPlaneError::Runtime(error) => ApiError::from_runtime(error, correlation_id),
        ControlPlaneError::SessionScopeMismatch { .. }
        | ControlPlaneError::LockWorkspaceMismatch { .. }
        | ControlPlaneError::LockAuthorityMismatch { .. } => ApiError::new(
            StatusCode::FORBIDDEN,
            "scope_denied",
            error.to_string(),
            correlation_id,
            None,
        ),
        ControlPlaneError::LockNotFound(_) => ApiError::new(
            StatusCode::NOT_FOUND,
            "lock_not_found",
            error.to_string(),
            correlation_id,
            None,
        ),
        ControlPlaneError::Execution(_)
        | ControlPlaneError::Orchestration(_)
        | ControlPlaneError::Knowledge(_) => ApiError::new(
            StatusCode::BAD_REQUEST,
            "control_plane_error",
            error.to_string(),
            correlation_id,
            None,
        ),
    }
}

#[derive(Debug, Deserialize)]
struct VersionRequest {
    expected_version: u64,
}

async fn register_agent(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(manifest): Json<AgentManifest>,
) -> Result<Json<AgentRecord>, ApiError> {
    let context = context();
    let session_id = parse_session(&session_id, context.correlation_id)?;
    state
        .control_plane
        .register_agent(session_id, manifest, &context)
        .await
        .map(Json)
        .map_err(|error| map_error(error, context.correlation_id))
}

async fn get_agent(
    State(state): State<AppState>,
    Path((session_id, agent_id)): Path<(String, String)>,
) -> Result<Json<AgentRecord>, ApiError> {
    let context = context();
    let session_id = parse_session(&session_id, context.correlation_id)?;
    let agent_id = parse_uuid(&agent_id, "agent ID", context.correlation_id)?;
    state
        .control_plane
        .get_agent(session_id, agent_id)
        .await
        .map(Json)
        .map_err(|error| map_error(error, context.correlation_id))
}

async fn start_agent(
    State(state): State<AppState>,
    Path((session_id, agent_id)): Path<(String, String)>,
    Json(request): Json<VersionRequest>,
) -> Result<Json<AgentRecord>, ApiError> {
    mutate_agent(state, session_id, agent_id, request.expected_version, "start").await
}

async fn heartbeat_agent(
    State(state): State<AppState>,
    Path((session_id, agent_id)): Path<(String, String)>,
    Json(request): Json<VersionRequest>,
) -> Result<Json<AgentRecord>, ApiError> {
    mutate_agent(
        state,
        session_id,
        agent_id,
        request.expected_version,
        "heartbeat",
    )
    .await
}

async fn stop_agent(
    State(state): State<AppState>,
    Path((session_id, agent_id)): Path<(String, String)>,
    Json(request): Json<VersionRequest>,
) -> Result<Json<AgentRecord>, ApiError> {
    mutate_agent(state, session_id, agent_id, request.expected_version, "stop").await
}

async fn mutate_agent(
    state: AppState,
    session_id: String,
    agent_id: String,
    expected_version: u64,
    operation: &'static str,
) -> Result<Json<AgentRecord>, ApiError> {
    let context = context();
    let session_id = parse_session(&session_id, context.correlation_id)?;
    let agent_id = parse_uuid(&agent_id, "agent ID", context.correlation_id)?;
    let result = match operation {
        "start" => {
            state
                .control_plane
                .start_agent(session_id, agent_id, expected_version, &context)
                .await
        }
        "heartbeat" => {
            state
                .control_plane
                .heartbeat_agent(session_id, agent_id, expected_version, &context)
                .await
        }
        "stop" => {
            state
                .control_plane
                .stop_agent(session_id, agent_id, expected_version, &context)
                .await
        }
        _ => unreachable!("known agent operation"),
    };
    result
        .map(Json)
        .map_err(|error| map_error(error, context.correlation_id))
}

async fn create_workflow(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(definition): Json<WorkflowDefinition>,
) -> Result<Json<WorkflowExecution>, ApiError> {
    let context = context();
    let session_id = parse_session(&session_id, context.correlation_id)?;
    state
        .control_plane
        .create_workflow(session_id, definition, &context)
        .await
        .map(Json)
        .map_err(|error| map_error(error, context.correlation_id))
}

async fn get_workflow(
    State(state): State<AppState>,
    Path((session_id, workflow_id)): Path<(String, String)>,
) -> Result<Json<WorkflowExecution>, ApiError> {
    let context = context();
    let session_id = parse_session(&session_id, context.correlation_id)?;
    let workflow_id = parse_uuid(&workflow_id, "workflow ID", context.correlation_id)?;
    state
        .control_plane
        .get_workflow(session_id, workflow_id)
        .await
        .map(Json)
        .map_err(|error| map_error(error, context.correlation_id))
}

#[derive(Debug, Deserialize)]
struct StartNodeRequest {
    expected_version: u64,
    owner_id: String,
}

#[derive(Debug, Deserialize)]
struct FailNodeRequest {
    expected_version: u64,
    error: String,
}

#[derive(Debug, Deserialize)]
struct ApprovalRequest {
    expected_version: u64,
    approved: bool,
}

async fn start_workflow_node(
    State(state): State<AppState>,
    Path((session_id, workflow_id, node_id)): Path<(String, String, String)>,
    Json(request): Json<StartNodeRequest>,
) -> Result<Json<WorkflowExecution>, ApiError> {
    let context = context();
    let session_id = parse_session(&session_id, context.correlation_id)?;
    let workflow_id = parse_uuid(&workflow_id, "workflow ID", context.correlation_id)?;
    state
        .control_plane
        .start_workflow_node(
            session_id,
            workflow_id,
            request.expected_version,
            &node_id,
            request.owner_id,
            &context,
        )
        .await
        .map(Json)
        .map_err(|error| map_error(error, context.correlation_id))
}

async fn complete_workflow_node(
    State(state): State<AppState>,
    Path((session_id, workflow_id, node_id)): Path<(String, String, String)>,
    Json(request): Json<VersionRequest>,
) -> Result<Json<WorkflowExecution>, ApiError> {
    let context = context();
    let session_id = parse_session(&session_id, context.correlation_id)?;
    let workflow_id = parse_uuid(&workflow_id, "workflow ID", context.correlation_id)?;
    state
        .control_plane
        .complete_workflow_node(
            session_id,
            workflow_id,
            request.expected_version,
            &node_id,
            &context,
        )
        .await
        .map(Json)
        .map_err(|error| map_error(error, context.correlation_id))
}

async fn fail_workflow_node(
    State(state): State<AppState>,
    Path((session_id, workflow_id, node_id)): Path<(String, String, String)>,
    Json(request): Json<FailNodeRequest>,
) -> Result<Json<WorkflowExecution>, ApiError> {
    let context = context();
    let session_id = parse_session(&session_id, context.correlation_id)?;
    let workflow_id = parse_uuid(&workflow_id, "workflow ID", context.correlation_id)?;
    state
        .control_plane
        .fail_workflow_node(
            session_id,
            workflow_id,
            request.expected_version,
            &node_id,
            request.error,
            &context,
        )
        .await
        .map(Json)
        .map_err(|error| map_error(error, context.correlation_id))
}

async fn decide_workflow_approval(
    State(state): State<AppState>,
    Path((session_id, workflow_id, node_id)): Path<(String, String, String)>,
    Json(request): Json<ApprovalRequest>,
) -> Result<Json<WorkflowExecution>, ApiError> {
    let context = context();
    let session_id = parse_session(&session_id, context.correlation_id)?;
    let workflow_id = parse_uuid(&workflow_id, "workflow ID", context.correlation_id)?;
    state
        .control_plane
        .decide_workflow_approval(
            session_id,
            workflow_id,
            request.expected_version,
            &node_id,
            request.approved,
            &context,
        )
        .await
        .map(Json)
        .map_err(|error| map_error(error, context.correlation_id))
}

#[derive(Debug, Deserialize)]
struct AcquireLockRequest {
    owner_id: String,
    resource: LockResource,
    mode: LockMode,
    ttl_seconds: u32,
}

#[derive(Debug, Deserialize)]
struct WorkspaceQuery {
    workspace_id: String,
}

#[derive(Debug, Deserialize)]
struct HeartbeatLockRequest {
    workspace_id: String,
    owner_id: String,
    fencing_token: u64,
    ttl_seconds: u32,
}

#[derive(Debug, Deserialize)]
struct ReleaseLockRequest {
    workspace_id: String,
    owner_id: String,
    fencing_token: u64,
}

async fn acquire_lock(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<AcquireLockRequest>,
) -> Result<Json<LockLease>, ApiError> {
    let context = context();
    let session_id = parse_session(&session_id, context.correlation_id)?;
    state
        .control_plane
        .acquire_lock(
            &LockRequest {
                session_id,
                owner_id: request.owner_id,
                resource: request.resource,
                mode: request.mode,
                ttl_seconds: request.ttl_seconds,
            },
            &context,
        )
        .await
        .map(Json)
        .map_err(|error| map_error(error, context.correlation_id))
}

async fn list_locks(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<WorkspaceQuery>,
) -> Result<Json<Vec<LockLease>>, ApiError> {
    let context = context();
    let session_id = parse_session(&session_id, context.correlation_id)?;
    state
        .control_plane
        .list_locks(session_id, &query.workspace_id)
        .await
        .map(Json)
        .map_err(|error| map_error(error, context.correlation_id))
}

async fn heartbeat_lock(
    State(state): State<AppState>,
    Path((session_id, lock_id)): Path<(String, String)>,
    Json(request): Json<HeartbeatLockRequest>,
) -> Result<Json<LockLease>, ApiError> {
    let context = context();
    let session_id = parse_session(&session_id, context.correlation_id)?;
    let lock_id = parse_uuid(&lock_id, "lock ID", context.correlation_id)?;
    state
        .control_plane
        .heartbeat_lock(
            session_id,
            &request.workspace_id,
            lock_id,
            &request.owner_id,
            request.fencing_token,
            request.ttl_seconds,
            &context,
        )
        .await
        .map(Json)
        .map_err(|error| map_error(error, context.correlation_id))
}

async fn release_lock(
    State(state): State<AppState>,
    Path((session_id, lock_id)): Path<(String, String)>,
    Json(request): Json<ReleaseLockRequest>,
) -> Result<StatusCode, ApiError> {
    let context = context();
    let session_id = parse_session(&session_id, context.correlation_id)?;
    let lock_id = parse_uuid(&lock_id, "lock ID", context.correlation_id)?;
    state
        .control_plane
        .release_lock(
            session_id,
            &request.workspace_id,
            lock_id,
            &request.owner_id,
            request.fencing_token,
            &context,
        )
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| map_error(error, context.correlation_id))
}

#[derive(Debug, Deserialize)]
struct RememberRequest {
    class: MemoryClass,
    content: String,
    source: MemorySource,
    #[serde(default)]
    tags: BTreeSet<String>,
}

#[derive(Debug, Deserialize)]
struct SearchMemoryRequest {
    text: String,
    #[serde(default)]
    classes: BTreeSet<MemoryClass>,
    #[serde(default)]
    tags: BTreeSet<String>,
    #[serde(default = "default_memory_limit")]
    limit: usize,
}

const fn default_memory_limit() -> usize {
    20
}

async fn remember(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<RememberRequest>,
) -> Result<Json<MemoryRecord>, ApiError> {
    let context = context();
    let session_id = parse_session(&session_id, context.correlation_id)?;
    let record = MemoryRecord::new(
        session_id,
        request.class,
        request.content,
        request.source,
        request.tags,
    )
    .map_err(|error| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "validation",
            error.to_string(),
            context.correlation_id,
            None,
        )
    })?;
    state
        .control_plane
        .remember(record, &context)
        .await
        .map(Json)
        .map_err(|error| map_error(error, context.correlation_id))
}

async fn search_memories(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<SearchMemoryRequest>,
) -> Result<Json<Vec<sessionweft_knowledge::MemoryHit>>, ApiError> {
    let context = context();
    let session_id = parse_session(&session_id, context.correlation_id)?;
    state
        .control_plane
        .search_memories(&MemoryQuery {
            session_id,
            text: request.text,
            classes: request.classes,
            tags: request.tags,
            limit: request.limit,
        })
        .await
        .map(Json)
        .map_err(|error| map_error(error, context.correlation_id))
}

async fn forget_memory(
    State(state): State<AppState>,
    Path((session_id, memory_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let context = context();
    let session_id = parse_session(&session_id, context.correlation_id)?;
    let memory_id = parse_uuid(&memory_id, "memory ID", context.correlation_id)?;
    state
        .control_plane
        .forget_memory(session_id, memory_id, &context)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| map_error(error, context.correlation_id))
}
