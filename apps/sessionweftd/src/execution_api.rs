use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;
use sessionweft_client_protocol::{
    AgentExecutionSupervisor, AgentExecutionView, ExecutionError, StartAgentExecutionRequest,
    StartAgentExecutionResponse, StopAgentExecutionRequest, TerminalFrameBatch,
    TerminalInputRequest, TerminalResizeRequest,
};
use sessionweft_core::SessionId;
use uuid::Uuid;

use crate::{ApiError, AppState};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/sessions/{session_id}/workflows/{workflow_id}/nodes/{node_id}/executions",
            post(start_execution),
        )
        .route("/v1/executions/{execution_id}", get(get_execution))
        .route("/v1/executions/{execution_id}/terminal", get(read_terminal))
        .route(
            "/v1/executions/{execution_id}/terminal/input",
            post(write_terminal),
        )
        .route(
            "/v1/executions/{execution_id}/terminal/resize",
            post(resize_terminal),
        )
        .route("/v1/executions/{execution_id}/stop", post(stop_execution))
}

async fn start_execution(
    State(state): State<AppState>,
    Path((session_id, workflow_id, node_id)): Path<(String, Uuid, String)>,
    Json(request): Json<StartAgentExecutionRequest>,
) -> Result<Json<StartAgentExecutionResponse>, ApiError> {
    let correlation_id = Uuid::new_v4();
    let session_id = session_id.parse::<SessionId>().map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_session_id",
            "session id must be a UUID",
            correlation_id,
            None,
        )
    })?;
    state
        .executions
        .start(session_id, workflow_id, node_id, request)
        .map(Json)
        .map_err(execution_error)
}

async fn get_execution(
    State(state): State<AppState>,
    Path(execution_id): Path<Uuid>,
) -> Result<Json<AgentExecutionView>, ApiError> {
    state
        .executions
        .view(execution_id)
        .map(Json)
        .map_err(execution_error)
}

#[derive(Debug, Deserialize)]
struct TerminalQuery {
    #[serde(default)]
    after: u64,
}

async fn read_terminal(
    State(state): State<AppState>,
    Path(execution_id): Path<Uuid>,
    Query(query): Query<TerminalQuery>,
) -> Result<Json<TerminalFrameBatch>, ApiError> {
    state
        .executions
        .terminal_after(execution_id, query.after)
        .map(Json)
        .map_err(execution_error)
}

async fn write_terminal(
    State(state): State<AppState>,
    Path(execution_id): Path<Uuid>,
    Json(request): Json<TerminalInputRequest>,
) -> Result<StatusCode, ApiError> {
    state
        .executions
        .input(execution_id, request)
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(execution_error)
}

async fn resize_terminal(
    State(state): State<AppState>,
    Path(execution_id): Path<Uuid>,
    Json(request): Json<TerminalResizeRequest>,
) -> Result<StatusCode, ApiError> {
    state
        .executions
        .resize(execution_id, request)
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(execution_error)
}

async fn stop_execution(
    State(state): State<AppState>,
    Path(execution_id): Path<Uuid>,
    Json(request): Json<StopAgentExecutionRequest>,
) -> Result<Json<AgentExecutionView>, ApiError> {
    state
        .executions
        .stop(execution_id, request)
        .map(Json)
        .map_err(execution_error)
}

fn execution_error(error: ExecutionError) -> ApiError {
    let correlation_id = Uuid::new_v4();
    let (status, code) = match error {
        ExecutionError::NotFound(_) => (StatusCode::NOT_FOUND, "execution_not_found"),
        ExecutionError::FencingTokenMismatch => (StatusCode::CONFLICT, "fencing_token_mismatch"),
        ExecutionError::Validation(_) => (StatusCode::BAD_REQUEST, "invalid_execution_request"),
        ExecutionError::Pty(_) => (StatusCode::BAD_GATEWAY, "pty_execution_failed"),
        ExecutionError::Poisoned | ExecutionError::Io(_) | ExecutionError::Json(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "execution_internal_error",
        ),
    };
    ApiError::new(status, code, error.to_string(), correlation_id, None)
}

#[allow(dead_code)]
fn _assert_supervisor_is_send_sync(_: &AgentExecutionSupervisor) {}
