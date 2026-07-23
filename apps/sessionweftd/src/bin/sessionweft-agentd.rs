#[path = "../standalone_agent.rs"]
mod standalone_agent;

use std::{collections::BTreeSet, env, net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{Context, bail};
use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sessionweft_client_protocol::{PtySupervisor, discover_programs};
use sessionweft_core::{DomainError, Session, SessionId};
use sessionweft_runtime::RuntimeError;
use sessionweft_storage::StorageError;
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use standalone_agent::{
    StandaloneAgentKind, StandaloneAgentManager, StandaloneAgentSendResult,
    StandaloneAgentSessionView, StandaloneRuntime, StartStandaloneAgentRequest, build_runtime,
};

#[derive(Clone)]
struct AppState {
    runtime: StandaloneRuntime,
    agents: Arc<StandaloneAgentManager>,
    api_token: Option<Arc<str>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("sessionweft=info,tower_http=info")),
        )
        .json()
        .init();

    let bind = env::var("SESSIONWEFT_AGENT_BIND").unwrap_or_else(|_| "127.0.0.1:7449".into());
    let address: SocketAddr = bind.parse().context("parse SESSIONWEFT_AGENT_BIND")?;
    let api_token = env::var("SESSIONWEFT_AGENT_API_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(Arc::<str>::from);
    if !address.ip().is_loopback() && api_token.is_none() {
        bail!("SESSIONWEFT_AGENT_API_TOKEN is required for a non-loopback bind");
    }

    let database_url =
        env::var("SESSIONWEFT_DATABASE_URL").unwrap_or_else(|_| "sqlite://sessionweft.db".into());
    let workspace_root = env::var_os("SESSIONWEFT_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .unwrap_or(env::current_dir().context("resolve standalone agent workspace root")?);
    let configured_programs = env::var("SESSIONWEFT_STANDALONE_AGENT_PROGRAMS")
        .unwrap_or_else(|_| "sh,bash,pwsh,cmd.exe,codex,claude,gemini,antigravity-ide".into());
    let program_names = configured_programs
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let programs = discover_programs(&program_names);
    let available_programs = programs.keys().cloned().collect::<BTreeSet<_>>();
    let pty = Arc::new(
        PtySupervisor::new(
            &workspace_root,
            programs,
            BTreeSet::from([
                "HOME".into(),
                "USER".into(),
                "LOGNAME".into(),
                "SHELL".into(),
                "PATH".into(),
                "XDG_CONFIG_HOME".into(),
                "XDG_DATA_HOME".into(),
                "XDG_CACHE_HOME".into(),
                "TERM".into(),
                "COLORTERM".into(),
                "LANG".into(),
                "LC_ALL".into(),
            ]),
        )
        .context("initialize standalone agent PTY supervisor")?,
    );
    let runtime = build_runtime(&database_url).await?;
    let agents = Arc::new(
        StandaloneAgentManager::connect(&database_url, pty, &workspace_root, available_programs)
            .await?,
    );
    let state = AppState {
        runtime,
        agents,
        api_token,
    };

    let protected = Router::new()
        .route("/health/ready", get(readiness))
        .route("/v1/sessions", get(list_sessions).post(create_session))
        .route("/v1/sessions/{session_id}", get(get_session))
        .route(
            "/v1/sessions/{session_id}/standalone-agents",
            get(agent_session_view),
        )
        .route(
            "/v1/sessions/{session_id}/standalone-agents/history",
            get(agent_history),
        )
        .route(
            "/v1/sessions/{session_id}/standalone-agents/context",
            get(agent_context),
        )
        .route(
            "/v1/sessions/{session_id}/standalone-agents/send",
            post(send_active_agent),
        )
        .route(
            "/v1/sessions/{session_id}/standalone-agents/{agent}/start",
            post(start_agent),
        )
        .route(
            "/v1/sessions/{session_id}/standalone-agents/{agent}/switch",
            post(switch_agent),
        )
        .route(
            "/v1/sessions/{session_id}/standalone-agents/{agent}/resume",
            post(resume_agent),
        )
        .route(
            "/v1/sessions/{session_id}/standalone-agents/{agent}/send",
            post(send_agent),
        )
        .route(
            "/v1/sessions/{session_id}/standalone-agents/{agent}/stop",
            post(stop_agent),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
    let app = protected
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("bind standalone agent daemon to {address}"))?;
    info!(%address, "SessionWeft standalone agent daemon started");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("standalone agent HTTP server failed")
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

async fn authenticate(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(expected) = state.api_token.as_deref() else {
        return next.run(request).await;
    };
    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|value| constant_time_eq(value.as_bytes(), expected.as_bytes()));
    if authorized {
        next.run(request).await
    } else {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "valid standalone agent bearer token required",
            Uuid::new_v4(),
        )
        .into_response()
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

async fn readiness(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ready",
        "mode": "standalone_agents",
        "agents": state.agents.availability(),
    }))
}

#[derive(Debug, Deserialize)]
struct ListSessionsQuery {
    #[serde(default = "default_session_limit")]
    limit: u32,
}

const fn default_session_limit() -> u32 {
    100
}

async fn list_sessions(
    State(state): State<AppState>,
    Query(query): Query<ListSessionsQuery>,
) -> Result<Json<Vec<Session>>, ApiError> {
    state
        .runtime
        .list_sessions(query.limit.clamp(1, 1_000))
        .await
        .map(Json)
        .map_err(runtime_error)
}

#[derive(Debug, Deserialize)]
struct CreateSessionRequest {
    title: String,
}

async fn create_session(
    State(state): State<AppState>,
    Json(request): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<Session>), ApiError> {
    state
        .runtime
        .create_session(request.title, Some("standalone-agent-api"), Uuid::new_v4())
        .await
        .map(|session| (StatusCode::CREATED, Json(session)))
        .map_err(runtime_error)
}

async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Session>, ApiError> {
    let session_id = parse_session(&session_id)?;
    state
        .runtime
        .get_session(session_id)
        .await
        .map(Json)
        .map_err(runtime_error)
}

async fn agent_session_view(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<StandaloneAgentSessionView>, ApiError> {
    let session_id = parse_session(&session_id)?;
    state
        .runtime
        .get_session(session_id)
        .await
        .map_err(runtime_error)?;
    state
        .agents
        .session_view(session_id)
        .await
        .map(Json)
        .map_err(agent_error)
}

async fn start_agent(
    State(state): State<AppState>,
    Path((session_id, agent)): Path<(String, String)>,
    Json(request): Json<StartStandaloneAgentRequest>,
) -> Result<
    (
        StatusCode,
        Json<standalone_agent::StandaloneAgentBindingView>,
    ),
    ApiError,
> {
    let session_id = parse_session(&session_id)?;
    let agent = parse_agent(&agent)?;
    let session = state
        .runtime
        .get_session(session_id)
        .await
        .map_err(runtime_error)?;
    state
        .agents
        .start_or_resume(state.runtime.clone(), &session, agent, request)
        .await
        .map(|binding| (StatusCode::CREATED, Json(binding)))
        .map_err(agent_error)
}

async fn resume_agent(
    State(state): State<AppState>,
    Path((session_id, agent)): Path<(String, String)>,
    body: Option<Json<StartStandaloneAgentRequest>>,
) -> Result<Json<standalone_agent::StandaloneAgentBindingView>, ApiError> {
    let session_id = parse_session(&session_id)?;
    let agent = parse_agent(&agent)?;
    let session = state
        .runtime
        .get_session(session_id)
        .await
        .map_err(runtime_error)?;
    state
        .agents
        .start_or_resume(
            state.runtime.clone(),
            &session,
            agent,
            body.map_or_else(StartStandaloneAgentRequest::default, |Json(value)| value),
        )
        .await
        .map(Json)
        .map_err(agent_error)
}

async fn switch_agent(
    State(state): State<AppState>,
    Path((session_id, agent)): Path<(String, String)>,
) -> Result<Json<standalone_agent::StandaloneAgentBindingView>, ApiError> {
    let session_id = parse_session(&session_id)?;
    let agent = parse_agent(&agent)?;
    let session = state
        .runtime
        .get_session(session_id)
        .await
        .map_err(runtime_error)?;
    state
        .agents
        .switch(state.runtime.clone(), &session, agent)
        .await
        .map(Json)
        .map_err(agent_error)
}

#[derive(Debug, Deserialize)]
struct SendAgentRequest {
    message: String,
}

async fn send_active_agent(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<SendAgentRequest>,
) -> Result<Json<StandaloneAgentSendResult>, ApiError> {
    let session_id = parse_session(&session_id)?;
    state
        .runtime
        .get_session(session_id)
        .await
        .map_err(runtime_error)?;
    state
        .agents
        .send(state.runtime.clone(), session_id, None, request.message)
        .await
        .map(Json)
        .map_err(agent_error)
}

async fn send_agent(
    State(state): State<AppState>,
    Path((session_id, agent)): Path<(String, String)>,
    Json(request): Json<SendAgentRequest>,
) -> Result<Json<StandaloneAgentSendResult>, ApiError> {
    let session_id = parse_session(&session_id)?;
    let agent = parse_agent(&agent)?;
    state
        .runtime
        .get_session(session_id)
        .await
        .map_err(runtime_error)?;
    state
        .agents
        .send(
            state.runtime.clone(),
            session_id,
            Some(agent),
            request.message,
        )
        .await
        .map(Json)
        .map_err(agent_error)
}

async fn stop_agent(
    State(state): State<AppState>,
    Path((session_id, agent)): Path<(String, String)>,
) -> Result<Json<standalone_agent::StandaloneAgentBindingView>, ApiError> {
    let session_id = parse_session(&session_id)?;
    let agent = parse_agent(&agent)?;
    state
        .runtime
        .get_session(session_id)
        .await
        .map_err(runtime_error)?;
    state
        .agents
        .stop(session_id, agent)
        .await
        .map(Json)
        .map_err(agent_error)
}

#[derive(Debug, Deserialize)]
struct HistoryQuery {
    #[serde(default)]
    after: u64,
    #[serde(default = "default_history_limit")]
    limit: u32,
}

const fn default_history_limit() -> u32 {
    100
}

async fn agent_history(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<standalone_agent::StandaloneAgentHistoryPage>, ApiError> {
    let session_id = parse_session(&session_id)?;
    state
        .runtime
        .get_session(session_id)
        .await
        .map_err(runtime_error)?;
    state
        .agents
        .history(session_id, query.after, query.limit)
        .await
        .map(Json)
        .map_err(agent_error)
}

#[derive(Debug, Serialize)]
struct ContextResponse {
    session_id: SessionId,
    path: String,
    content: String,
}

async fn agent_context(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<ContextResponse>, ApiError> {
    let session_id = parse_session(&session_id)?;
    let session = state
        .runtime
        .get_session(session_id)
        .await
        .map_err(runtime_error)?;
    let (path, content) = state.agents.context(&session).await.map_err(agent_error)?;
    Ok(Json(ContextResponse {
        session_id,
        path,
        content,
    }))
}

fn parse_session(value: &str) -> Result<SessionId, ApiError> {
    value.parse().map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_session_id",
            "session ID must be a UUID",
            Uuid::new_v4(),
        )
    })
}

fn parse_agent(value: &str) -> Result<StandaloneAgentKind, ApiError> {
    value.parse().map_err(agent_error)
}

fn runtime_error(error: RuntimeError) -> ApiError {
    let correlation_id = Uuid::new_v4();
    match error {
        RuntimeError::Domain(DomainError::Validation(message)) => ApiError::new(
            StatusCode::BAD_REQUEST,
            "validation",
            message,
            correlation_id,
        ),
        RuntimeError::Domain(DomainError::Conflict { expected, actual }) => ApiError::new(
            StatusCode::CONFLICT,
            "conflict",
            format!("expected version {expected}, actual version {actual}"),
            correlation_id,
        ),
        RuntimeError::Domain(DomainError::Archived) => ApiError::new(
            StatusCode::CONFLICT,
            "session_archived",
            "session is archived",
            correlation_id,
        ),
        RuntimeError::Storage(StorageError::Conflict { expected, actual }) => ApiError::new(
            StatusCode::CONFLICT,
            "conflict",
            format!("expected version {expected}, actual version {actual}"),
            correlation_id,
        ),
        RuntimeError::NotFound(_) | RuntimeError::Storage(StorageError::NotFound(_)) => {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "session_not_found",
                "session not found",
                correlation_id,
            )
        }
        other => ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_error",
            other.to_string(),
            correlation_id,
        ),
    }
}

fn agent_error(error: impl std::fmt::Display) -> ApiError {
    let message = error.to_string();
    let status = if message.contains("not found in PATH")
        || message.contains("must be resumed")
        || message.contains("not running")
        || message.contains("no active standalone agent")
        || message.contains("unsupported standalone agent")
        || message.contains("has not been started")
    {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    ApiError::new(status, "standalone_agent_error", message, Uuid::new_v4())
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
    correlation_id: Uuid,
}

struct ApiError {
    status: StatusCode,
    body: ErrorBody,
}

impl ApiError {
    fn new(
        status: StatusCode,
        code: &'static str,
        message: impl Into<String>,
        correlation_id: Uuid,
    ) -> Self {
        Self {
            status,
            body: ErrorBody {
                code,
                message: message.into(),
                correlation_id,
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}
