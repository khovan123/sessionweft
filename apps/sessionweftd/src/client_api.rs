use std::{convert::Infallible, time::Duration};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
};
use futures_util::Stream;
use serde::Deserialize;
use sessionweft_client_protocol::{
    ApiEnvelope, ClientResourceView, EventCursor, EventJournal, PendingApproval,
    ProtocolCapabilities, PtyInputRequest, PtyResizeRequest, StartPtyRequest,
};
use sessionweft_orchestration::{WorkflowExecution, WorkflowNodeKind, WorkflowNodeStatus};
use uuid::Uuid;

use super::{ApiError, AppState, parse_session_id};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/client/protocol", get(protocol))
        .route("/v1/events", get(list_events))
        .route("/v1/events/stream", get(stream_events))
        .route(
            "/v1/sessions/{session_id}/client-view",
            get(client_view),
        )
        .route("/v1/pty", post(start_pty))
        .route("/v1/pty/{pty_id}", get(get_pty))
        .route("/v1/pty/{pty_id}/input", post(pty_input))
        .route("/v1/pty/{pty_id}/resize", post(pty_resize))
        .route("/v1/pty/{pty_id}/cancel", post(pty_cancel))
        .route("/v1/pty/{pty_id}/output", get(pty_output))
}

async fn protocol(State(state): State<AppState>) -> Json<ApiEnvelope<ProtocolCapabilities>> {
    Json(ApiEnvelope::new(
        Uuid::new_v4(),
        ProtocolCapabilities {
            authenticated: state.api_token.is_some(),
            ..Default::default()
        },
    ))
}

#[derive(Debug, Deserialize)]
struct EventQuery {
    #[serde(default)]
    after: u64,
    #[serde(default = "default_event_limit")]
    limit: u32,
}

const fn default_event_limit() -> u32 {
    sessionweft_client_protocol::DEFAULT_EVENT_BATCH_LIMIT
}

async fn list_events(
    State(state): State<AppState>,
    Query(query): Query<EventQuery>,
) -> Result<Json<ApiEnvelope<sessionweft_client_protocol::EventBatch>>, ApiError> {
    let correlation_id = Uuid::new_v4();
    let batch = state
        .event_journal
        .list_after(EventCursor(query.after), query.limit)
        .await
        .map_err(|error| internal_error(error, correlation_id))?;
    Ok(Json(ApiEnvelope::new(correlation_id, batch)))
}

async fn stream_events(
    State(state): State<AppState>,
    Query(query): Query<EventQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let journal = state.event_journal;
    let stream = async_stream::stream! {
        let mut cursor = EventCursor(query.after);
        loop {
            match journal.list_after(cursor, query.limit).await {
                Ok(batch) if batch.events.is_empty() => {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Ok(batch) => {
                    for record in batch.events {
                        cursor = record.cursor;
                        let event = Event::default()
                            .id(record.cursor.0.to_string())
                            .event(record.envelope.event_type.clone())
                            .json_data(record)
                            .unwrap_or_else(|error| {
                                Event::default()
                                    .event("client.serialization_error")
                                    .data(error.to_string())
                            });
                        yield Ok(event);
                    }
                }
                Err(error) => {
                    yield Ok(Event::default().event("client.stream_error").data(error.to_string()));
                    break;
                }
            }
        }
    };
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("sessionweft"),
    )
}

#[derive(Debug, Deserialize)]
struct ClientViewQuery {
    agent_id: Option<Uuid>,
    workflow_id: Option<Uuid>,
    workspace_id: Option<String>,
}

async fn client_view(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<ClientViewQuery>,
) -> Result<Json<ApiEnvelope<ClientResourceView>>, ApiError> {
    let correlation_id = Uuid::new_v4();
    let session_id = parse_session_id(&session_id, correlation_id)?;
    let session = state
        .runtime
        .get_session(session_id)
        .await
        .map_err(|error| ApiError::from_runtime(error, correlation_id))?;
    let agent = if let Some(agent_id) = query.agent_id {
        Some(
            state
                .control_plane
                .get_agent(session_id, agent_id)
                .await
                .map_err(|error| control_error(error, correlation_id))?,
        )
    } else {
        None
    };
    let workflow = if let Some(workflow_id) = query.workflow_id {
        Some(
            state
                .control_plane
                .get_workflow(session_id, workflow_id)
                .await
                .map_err(|error| control_error(error, correlation_id))?,
        )
    } else {
        None
    };
    let locks = if let Some(workspace_id) = query.workspace_id.as_deref() {
        state
            .control_plane
            .list_locks(session_id, workspace_id)
            .await
            .map_err(|error| control_error(error, correlation_id))?
    } else {
        Vec::new()
    };
    let pending_approvals = workflow
        .as_ref()
        .map(pending_approvals)
        .unwrap_or_default();
    let view = ClientResourceView {
        protocol_version: sessionweft_client_protocol::CLIENT_PROTOCOL_VERSION,
        session_id,
        session: serde_json::to_value(session)
            .map_err(|error| internal_error(error, correlation_id))?,
        agent: agent
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| internal_error(error, correlation_id))?,
        workflow: workflow
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| internal_error(error, correlation_id))?,
        locks: locks
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| internal_error(error, correlation_id))?,
        pending_approvals,
        generated_at: chrono::Utc::now(),
    };
    Ok(Json(ApiEnvelope::new(correlation_id, view)))
}

fn pending_approvals(workflow: &WorkflowExecution) -> Vec<PendingApproval> {
    workflow
        .definition
        .nodes
        .iter()
        .filter(|definition| definition.kind == WorkflowNodeKind::Approval)
        .filter_map(|definition| {
            let state = workflow.nodes.get(&definition.id)?;
            (state.status == WorkflowNodeStatus::WaitingApproval).then(|| PendingApproval {
                workflow_id: workflow.id,
                node_id: definition.id.clone(),
                expected_version: workflow.version,
                title: definition.id.clone(),
                reason: state.last_error.clone(),
            })
        })
        .collect()
}

async fn start_pty(
    State(state): State<AppState>,
    Json(request): Json<StartPtyRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<sessionweft_client_protocol::PtySessionDescriptor>>), ApiError>
{
    let correlation_id = Uuid::new_v4();
    state
        .runtime
        .get_session(request.session_id)
        .await
        .map_err(|error| ApiError::from_runtime(error, correlation_id))?;
    let descriptor = state
        .pty
        .start(request)
        .map_err(|error| pty_error(error, correlation_id))?;
    Ok((
        StatusCode::CREATED,
        Json(ApiEnvelope::new(correlation_id, descriptor)),
    ))
}

async fn get_pty(
    State(state): State<AppState>,
    Path(pty_id): Path<Uuid>,
) -> Result<Json<ApiEnvelope<sessionweft_client_protocol::PtySessionDescriptor>>, ApiError> {
    let correlation_id = Uuid::new_v4();
    let descriptor = state
        .pty
        .descriptor(pty_id)
        .map_err(|error| pty_error(error, correlation_id))?;
    Ok(Json(ApiEnvelope::new(correlation_id, descriptor)))
}

async fn pty_input(
    State(state): State<AppState>,
    Path(pty_id): Path<Uuid>,
    Json(request): Json<PtyInputRequest>,
) -> Result<StatusCode, ApiError> {
    let correlation_id = Uuid::new_v4();
    state
        .pty
        .input(pty_id, &request.data)
        .map_err(|error| pty_error(error, correlation_id))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn pty_resize(
    State(state): State<AppState>,
    Path(pty_id): Path<Uuid>,
    Json(request): Json<PtyResizeRequest>,
) -> Result<StatusCode, ApiError> {
    let correlation_id = Uuid::new_v4();
    state
        .pty
        .resize(pty_id, request)
        .map_err(|error| pty_error(error, correlation_id))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn pty_cancel(
    State(state): State<AppState>,
    Path(pty_id): Path<Uuid>,
) -> Result<Json<ApiEnvelope<sessionweft_client_protocol::PtySessionDescriptor>>, ApiError> {
    let correlation_id = Uuid::new_v4();
    let descriptor = state
        .pty
        .cancel(pty_id)
        .map_err(|error| pty_error(error, correlation_id))?;
    Ok(Json(ApiEnvelope::new(correlation_id, descriptor)))
}

#[derive(Debug, Deserialize)]
struct PtyOutputQuery {
    #[serde(default)]
    after: u64,
    #[serde(default = "default_wait_ms")]
    wait_ms: u64,
}

const fn default_wait_ms() -> u64 {
    1_000
}

async fn pty_output(
    State(state): State<AppState>,
    Path(pty_id): Path<Uuid>,
    Query(query): Query<PtyOutputQuery>,
) -> Result<Json<ApiEnvelope<sessionweft_client_protocol::PtyOutputBatch>>, ApiError> {
    let correlation_id = Uuid::new_v4();
    let output = state
        .pty
        .wait_for_output(
            pty_id,
            query.after,
            Duration::from_millis(query.wait_ms.min(30_000)),
        )
        .await
        .map_err(|error| pty_error(error, correlation_id))?;
    Ok(Json(ApiEnvelope::new(correlation_id, output)))
}

fn internal_error(error: impl std::fmt::Display, correlation_id: Uuid) -> ApiError {
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "client_protocol_error",
        error.to_string(),
        correlation_id,
        None,
    )
}

fn control_error(error: impl std::fmt::Display, correlation_id: Uuid) -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "client_resource_error",
        error.to_string(),
        correlation_id,
        None,
    )
}

fn pty_error(error: impl std::fmt::Display, correlation_id: Uuid) -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "pty_error",
        error.to_string(),
        correlation_id,
        None,
    )
}
