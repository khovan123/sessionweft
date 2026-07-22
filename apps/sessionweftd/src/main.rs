mod client_api;
mod control_plane_api;

use std::{
    collections::BTreeSet,
    env,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sessionweft_client_protocol::{
    CLIENT_PROTOCOL_VERSION, EventJournal, JournalEventTransport, PtySupervisor, discover_programs,
};
use sessionweft_client_protocol_sqlite::SqliteClientEventJournal;
use sessionweft_control_plane::RuntimeControlPlane;
use sessionweft_core::{DomainError, EventEnvelope, MessageRole, Session, SessionId};
use sessionweft_execution_sqlite::SqliteAgentRepository;
use sessionweft_knowledge_sqlite::SqliteMemoryRepository;
use sessionweft_observability::MetricsRegistry;
use sessionweft_orchestration_sqlite::SqliteOrchestrationRepository;
use sessionweft_provider::{EchoProvider, OllamaProvider, ProviderError, ProviderRegistry};
use sessionweft_runtime::{
    LocalEventTransport, RuntimeError, RuntimeService, run_outbox_publisher,
};
use sessionweft_storage::{SqliteSessionRepository, StorageError};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

type LocalControlPlane = RuntimeControlPlane<
    SqliteSessionRepository,
    SqliteAgentRepository,
    SqliteOrchestrationRepository,
    SqliteMemoryRepository,
>;

#[derive(Clone)]
struct AppState {
    runtime: RuntimeService<SqliteSessionRepository>,
    control_plane: Arc<LocalControlPlane>,
    providers: Arc<ProviderRegistry>,
    api_token: Option<Arc<str>>,
    event_journal: Arc<SqliteClientEventJournal>,
    pty: Arc<PtySupervisor>,
    metrics: Arc<MetricsRegistry>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let bind = env::var("SESSIONWEFT_BIND").unwrap_or_else(|_| "127.0.0.1:7447".into());
    let address: SocketAddr = bind.parse().context("invalid SESSIONWEFT_BIND")?;
    let api_token = env::var("SESSIONWEFT_API_TOKEN")
        .ok()
        .filter(|value| !value.is_empty())
        .map(Arc::<str>::from);
    if !address.ip().is_loopback() && api_token.is_none() {
        bail!("SESSIONWEFT_API_TOKEN is required for a non-loopback bind");
    }

    let database_url =
        env::var("SESSIONWEFT_DATABASE_URL").unwrap_or_else(|_| "sqlite://sessionweft.db".into());
    let repository = Arc::new(
        SqliteSessionRepository::connect(&database_url)
            .await
            .context("failed to initialize Session repository")?,
    );
    let event_journal = Arc::new(
        SqliteClientEventJournal::connect(&database_url)
            .await
            .context("failed to initialize client event journal")?,
    );

    let mut providers = ProviderRegistry::new();
    providers.register(EchoProvider);
    let ollama_url =
        env::var("SESSIONWEFT_OLLAMA_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
    providers.register(
        OllamaProvider::new(ollama_url, Duration::from_secs(120))
            .context("failed to initialize Ollama provider")?,
    );
    let providers = Arc::new(providers);
    let runtime = RuntimeService::new(Arc::clone(&repository), Arc::clone(&providers));
    let agent_repository = Arc::new(
        SqliteAgentRepository::connect(&database_url)
            .await
            .context("failed to initialize Agent repository")?,
    );
    let orchestration_repository = Arc::new(
        SqliteOrchestrationRepository::connect(&database_url)
            .await
            .context("failed to initialize Orchestration repository")?,
    );
    let memory_repository = Arc::new(
        SqliteMemoryRepository::connect(&database_url)
            .await
            .context("failed to initialize Memory repository")?,
    );
    let control_plane = Arc::new(RuntimeControlPlane::new(
        runtime.clone(),
        agent_repository,
        orchestration_repository,
        memory_repository,
    ));

    let workspace_root = env::var_os("SESSIONWEFT_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .unwrap_or(env::current_dir().context("failed to resolve Runtime workspace root")?);
    let configured_programs = env::var("SESSIONWEFT_PTY_PROGRAMS")
        .unwrap_or_else(|_| "bash,sh,pwsh,powershell.exe,cmd.exe".into());
    let program_names = configured_programs
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let pty = Arc::new(
        PtySupervisor::new(
            workspace_root,
            discover_programs(&program_names),
            BTreeSet::from([
                "COLORTERM".into(),
                "LANG".into(),
                "LC_ALL".into(),
                "TERM".into(),
            ]),
        )
        .context("failed to initialize Runtime PTY supervisor")?,
    );

    let local_transport = Arc::new(LocalEventTransport::new(1_024));
    let transport = Arc::new(JournalEventTransport::new(
        Arc::clone(&event_journal),
        local_transport,
    ));
    let cancellation = CancellationToken::new();
    let outbox_task = tokio::spawn(run_outbox_publisher(
        Arc::clone(&repository),
        transport,
        cancellation.clone(),
        Duration::from_millis(100),
    ));

    let metrics_registry = Arc::new(MetricsRegistry::new());
    let state = AppState {
        runtime,
        control_plane,
        providers,
        api_token,
        event_journal,
        pty,
        metrics: metrics_registry,
    };
    let protected = Router::new()
        .route("/health/ready", get(readiness))
        .route("/metrics", get(metrics))
        .route("/v1/sessions", get(list_sessions).post(create_session))
        .route("/v1/sessions/{id}", get(get_session))
        .route("/v1/sessions/{id}/messages", post(append_message))
        .route("/v1/sessions/{id}/provider", post(select_provider))
        .route("/v1/sessions/{id}/run", post(run_provider))
        .route("/v1/sessions/{id}/archive", post(archive_session))
        .merge(control_plane_api::routes())
        .merge(client_api::routes())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            audit_mutations,
        ))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
    let app = Router::new()
        .route("/health/live", get(liveness))
        .merge(protected)
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            observe_requests,
        ))
        .with_state(state);

    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("failed to bind {address}"))?;
    info!(
        operation = "server_start",
        bind = %address,
        protocol_version = CLIENT_PROTOCOL_VERSION,
        "SessionWeft Runtime started"
    );

    let shutdown = shutdown_signal(cancellation.clone());
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await;
    cancellation.cancel();
    if let Err(error) = outbox_task.await {
        warn!(operation = "outbox_join", error = %error, "outbox task join failed");
    }
    result.context("HTTP server failed")
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .with_current_span(true)
        .with_span_list(true)
        .init();
}

async fn shutdown_signal(cancellation: CancellationToken) {
    if let Err(error) = tokio::signal::ctrl_c().await {
        warn!(operation = "shutdown_signal", error = %error, "failed to listen for Ctrl+C");
    }
    cancellation.cancel();
}

async fn observe_requests(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let method = request.method().as_str().to_owned();
    let started = Instant::now();
    let response = next.run(request).await;
    state
        .metrics
        .record_http(&method, response.status().as_u16(), started.elapsed());
    response
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        state.metrics.render_prometheus(),
    )
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
    if !authorized {
        state.metrics.record_auth_denied();
        let correlation_id = Uuid::new_v4();
        warn!(operation = "authorize", correlation_id = %correlation_id, "request denied");
        return ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "valid bearer token required",
            correlation_id,
            None,
        )
        .into_response();
    }

    next.run(request).await
}

async fn audit_mutations(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let response = next.run(request).await;
    if method != Method::GET && response.status().is_success() {
        state.metrics.record_successful_mutation();
        let event = EventEnvelope::new(
            "client.command_completed",
            session_id_from_path(&path),
            Uuid::new_v4(),
            Some("api"),
            serde_json::json!({
                "method": method.as_str(),
                "path": path,
                "status": response.status().as_u16(),
            }),
        );
        if let Err(error) = state.event_journal.append(&event).await {
            state.metrics.record_event_journal_failure();
            warn!(operation = "client_command_audit", error = %error, "failed to journal client command");
        }
    }
    response
}

fn session_id_from_path(path: &str) -> Option<SessionId> {
    let mut parts = path.split('/');
    while let Some(part) = parts.next() {
        if part == "sessions" {
            return parts.next().and_then(|value| value.parse().ok());
        }
    }
    None
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

async fn liveness() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "live",
        "protocol_version": CLIENT_PROTOCOL_VERSION,
    }))
}

async fn readiness(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ready",
        "protocol_version": CLIENT_PROTOCOL_VERSION,
        "providers": state.providers.names(),
    }))
}

#[derive(Debug, Deserialize)]
struct CreateSessionRequest {
    title: String,
}

async fn create_session(
    State(state): State<AppState>,
    Json(request): Json<CreateSessionRequest>,
) -> Result<Json<Session>, ApiError> {
    let correlation_id = Uuid::new_v4();
    state
        .runtime
        .create_session(request.title, Some("api"), correlation_id)
        .await
        .map(Json)
        .map_err(|error| ApiError::from_runtime(error, correlation_id))
}

#[derive(Debug, Deserialize)]
struct ListSessionsQuery {
    limit: Option<u32>,
}

async fn list_sessions(
    State(state): State<AppState>,
    Query(query): Query<ListSessionsQuery>,
) -> Result<Json<Vec<Session>>, ApiError> {
    let correlation_id = Uuid::new_v4();
    state
        .runtime
        .list_sessions(query.limit.unwrap_or(100))
        .await
        .map(Json)
        .map_err(|error| ApiError::from_runtime(error, correlation_id))
}

async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Session>, ApiError> {
    let correlation_id = Uuid::new_v4();
    let session_id = parse_session_id(&id, correlation_id)?;
    state
        .runtime
        .get_session(session_id)
        .await
        .map(Json)
        .map_err(|error| ApiError::from_runtime(error, correlation_id))
}

#[derive(Debug, Deserialize)]
struct AppendMessageRequest {
    expected_version: u64,
    role: MessageRole,
    content: String,
}

async fn append_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<AppendMessageRequest>,
) -> Result<Json<Session>, ApiError> {
    let correlation_id = Uuid::new_v4();
    let session_id = parse_session_id(&id, correlation_id)?;
    state
        .runtime
        .append_message(
            session_id,
            request.expected_version,
            request.role,
            request.content,
            Some("api"),
            correlation_id,
        )
        .await
        .map(Json)
        .map_err(|error| ApiError::from_runtime(error, correlation_id))
}

#[derive(Debug, Deserialize)]
struct SelectProviderRequest {
    expected_version: u64,
    provider: String,
    model: String,
}

async fn select_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SelectProviderRequest>,
) -> Result<Json<Session>, ApiError> {
    let correlation_id = Uuid::new_v4();
    let session_id = parse_session_id(&id, correlation_id)?;
    state
        .runtime
        .select_provider(
            session_id,
            request.expected_version,
            request.provider,
            request.model,
            Some("api"),
            correlation_id,
        )
        .await
        .map(Json)
        .map_err(|error| ApiError::from_runtime(error, correlation_id))
}

#[derive(Debug, Deserialize)]
struct RunProviderRequest {
    expected_version: u64,
    input: String,
}

async fn run_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<RunProviderRequest>,
) -> Result<Json<Session>, ApiError> {
    let correlation_id = Uuid::new_v4();
    let session_id = parse_session_id(&id, correlation_id)?;
    state
        .runtime
        .run_provider(
            session_id,
            request.expected_version,
            request.input,
            Some("api"),
            correlation_id,
        )
        .await
        .map(Json)
        .map_err(|error| ApiError::from_runtime(error, correlation_id))
}

#[derive(Debug, Deserialize)]
struct ArchiveSessionRequest {
    expected_version: u64,
}

async fn archive_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ArchiveSessionRequest>,
) -> Result<Json<Session>, ApiError> {
    let correlation_id = Uuid::new_v4();
    let session_id = parse_session_id(&id, correlation_id)?;
    state
        .runtime
        .archive_session(
            session_id,
            request.expected_version,
            Some("api"),
            correlation_id,
        )
        .await
        .map(Json)
        .map_err(|error| ApiError::from_runtime(error, correlation_id))
}

pub(crate) fn parse_session_id(value: &str, correlation_id: Uuid) -> Result<SessionId, ApiError> {
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

#[derive(Debug, Serialize)]
struct ErrorBody {
    protocol_version: u32,
    code: &'static str,
    message: String,
    correlation_id: Uuid,
    retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    committed_version: Option<u64>,
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
        committed_version: Option<u64>,
    ) -> Self {
        Self {
            status,
            body: ErrorBody {
                protocol_version: CLIENT_PROTOCOL_VERSION,
                code,
                message: message.into(),
                correlation_id,
                retryable: status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS,
                committed_version,
            },
        }
    }

    fn from_runtime(error: RuntimeError, correlation_id: Uuid) -> Self {
        match error {
            RuntimeError::Domain(DomainError::Validation(message)) => Self::new(
                StatusCode::BAD_REQUEST,
                "validation",
                message,
                correlation_id,
                None,
            ),
            RuntimeError::Domain(DomainError::Conflict { expected, actual }) => Self::new(
                StatusCode::CONFLICT,
                "conflict",
                format!("expected version {expected}, actual version {actual}"),
                correlation_id,
                None,
            ),
            RuntimeError::Domain(DomainError::Archived) => Self::new(
                StatusCode::CONFLICT,
                "session_archived",
                "session is archived",
                correlation_id,
                None,
            ),
            RuntimeError::Storage(StorageError::Conflict { expected, actual }) => Self::new(
                StatusCode::CONFLICT,
                "conflict",
                format!("expected version {expected}, actual version {actual}"),
                correlation_id,
                None,
            ),
            RuntimeError::NotFound(_) | RuntimeError::Storage(StorageError::NotFound(_)) => {
                Self::new(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    "session not found",
                    correlation_id,
                    None,
                )
            }
            RuntimeError::ProviderNotSelected => Self::new(
                StatusCode::CONFLICT,
                "provider_not_selected",
                "select a provider before running the session",
                correlation_id,
                None,
            ),
            RuntimeError::Provider(ProviderError::NotRegistered(provider)) => Self::new(
                StatusCode::BAD_REQUEST,
                "provider_not_registered",
                format!("provider '{provider}' is not registered"),
                correlation_id,
                None,
            ),
            RuntimeError::ProviderAfterCommit {
                committed_version,
                source,
            } => Self::new(
                StatusCode::BAD_GATEWAY,
                "provider_failed_after_commit",
                source.to_string(),
                correlation_id,
                Some(committed_version),
            ),
            RuntimeError::Provider(error) => Self::new(
                StatusCode::BAD_GATEWAY,
                "provider_error",
                error.to_string(),
                correlation_id,
                None,
            ),
            RuntimeError::Storage(error) => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage_error",
                error.to_string(),
                correlation_id,
                None,
            ),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}
