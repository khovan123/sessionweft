use std::{collections::BTreeMap, env, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sessionweft_billing::{
    BillingPlan, BillingRepository, BillingService, MeterName, PlanId,
};
use sessionweft_billing_stripe::{StripeBillingConfig, StripeBillingProvider};
use sessionweft_control_plane::OperationContext;
use sessionweft_core::{Session, SessionId};
use sessionweft_execution::{AgentManifest, AgentRecord};
use sessionweft_orchestration::{LockLease, LockRequest, WorkflowDefinition, WorkflowExecution};
use sessionweft_provider::{EchoProvider, OllamaProvider, ProviderRegistry};
use sessionweft_saas_postgres::{
    PostgresBillingRepository, PostgresTenantAuthRepository, PostgresTenantRepository,
    SaasPostgresDatabase,
};
use sessionweft_tenancy::{
    PrincipalId, QuotaDimension, TenantContext, TenantId, TenantQuota, TenantRepository,
    TenantRole, TenantService,
};
use sessionweft_tenant_runtime::TenantRuntimeManager;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tower_http::{request_id::MakeRequestUuid, trace::TraceLayer};
use tracing::info;
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    bootstrap_hash: [u8; 32],
    tenants: Arc<TenantService<PostgresTenantRepository>>,
    tenant_repository: Arc<PostgresTenantRepository>,
    auth: Arc<PostgresTenantAuthRepository>,
    database: SaasPostgresDatabase,
    runtimes: Arc<TenantRuntimeManager>,
    stripe: Option<Arc<StripeBillingProvider>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sessionweft=info,tower_http=info".into()),
        )
        .json()
        .init();

    let database_url = required_env("SESSIONWEFT_SAAS_DATABASE_URL")?;
    let bootstrap_token = required_env("SESSIONWEFT_SAAS_BOOTSTRAP_TOKEN")?;
    if bootstrap_token.len() < 24 {
        anyhow::bail!("SESSIONWEFT_SAAS_BOOTSTRAP_TOKEN must contain at least 24 bytes");
    }
    let instance_id = env::var("SESSIONWEFT_INSTANCE_ID")
        .unwrap_or_else(|_| format!("saasd-{}", Uuid::new_v4()));
    let database = SaasPostgresDatabase::connect(&database_url)
        .await
        .context("connect SaaS authority database")?;
    let tenant_repository = Arc::new(PostgresTenantRepository::new(database.clone()));
    let tenants = Arc::new(TenantService::new(Arc::clone(&tenant_repository)));
    let auth = Arc::new(
        PostgresTenantAuthRepository::new(database.clone())
            .await
            .context("initialize tenant token authority")?,
    );
    let providers = Arc::new(provider_registry()?);
    let runtimes = Arc::new(TenantRuntimeManager::new(
        &database_url,
        instance_id,
        providers,
    )?);
    let stripe = stripe_provider()?;
    let state = AppState {
        bootstrap_hash: Sha256::digest(bootstrap_token.as_bytes()).into(),
        tenants,
        tenant_repository,
        auth,
        database,
        runtimes,
        stripe,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/v2/bootstrap/tenants", post(bootstrap_tenant))
        .route("/v2/tenants/{tenant_id}/tokens", post(issue_token))
        .route("/v2/tenants/{tenant_id}/quotas", put(set_quota))
        .route(
            "/v2/tenants/{tenant_id}/sessions",
            post(create_session),
        )
        .route(
            "/v2/tenants/{tenant_id}/sessions/{session_id}",
            get(get_session),
        )
        .route(
            "/v2/tenants/{tenant_id}/sessions/{session_id}/agents",
            post(register_agent),
        )
        .route(
            "/v2/tenants/{tenant_id}/sessions/{session_id}/workflows",
            post(create_workflow),
        )
        .route(
            "/v2/tenants/{tenant_id}/sessions/{session_id}/locks",
            get(list_locks).post(acquire_lock),
        )
        .route("/v2/tenants/{tenant_id}/billing/plans", put(upsert_plan))
        .route(
            "/v2/tenants/{tenant_id}/billing/subscriptions",
            post(create_subscription),
        )
        .route(
            "/v2/tenants/{tenant_id}/billing/entitlements/{name}",
            get(get_entitlement),
        )
        .route(
            "/v2/tenants/{tenant_id}/billing/usage",
            post(record_usage),
        )
        .route("/v2/billing/stripe/webhook", post(stripe_webhook))
        .with_state(state)
        .layer(TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
            tracing::info_span!(
                "http_request",
                method = %request.method(),
                path = %request.uri().path(),
            )
        }))
        .layer(axum::middleware::from_fn(request_id));

    let address = env::var("SESSIONWEFT_SAAS_BIND")
        .unwrap_or_else(|_| "127.0.0.1:7448".into())
        .parse::<SocketAddr>()
        .context("parse SESSIONWEFT_SAAS_BIND")?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    info!(%address, "SessionWeft SaaS Runtime listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn request_id(
    mut request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    if request.extensions().get::<Uuid>().is_none() {
        request.extensions_mut().insert(Uuid::new_v4());
    }
    next.run(request).await
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "ok", "mode": "multi_tenant_saas"}))
}

#[derive(Debug, Deserialize)]
struct BootstrapTenantRequest {
    slug: String,
    display_name: String,
    owner_principal_id: String,
    token_label: Option<String>,
}

#[derive(Debug, Serialize)]
struct BootstrapTenantResponse {
    tenant_id: TenantId,
    owner_principal_id: String,
    api_token: String,
}

async fn bootstrap_tenant(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BootstrapTenantRequest>,
) -> ApiResult<Json<BootstrapTenantResponse>> {
    require_bootstrap(&state, &headers)?;
    let owner_principal = PrincipalId::parse(request.owner_principal_id).map_err(ApiError::bad_request)?;
    let (tenant, owner) = state
        .tenants
        .bootstrap(request.slug, request.display_name, owner_principal)
        .await
        .map_err(ApiError::from_tenancy)?;
    for (dimension, hard_limit) in default_quotas() {
        state
            .tenant_repository
            .set_quota(&TenantQuota {
                tenant_id: tenant.id,
                dimension,
                hard_limit,
                updated_at: Utc::now(),
            })
            .await
            .map_err(ApiError::from_tenancy)?;
    }
    let token = state
        .auth
        .issue(
            tenant.id,
            owner.principal_id.clone(),
            request.token_label.unwrap_or_else(|| "bootstrap-owner".into()),
            None,
        )
        .await
        .map_err(ApiError::from_tenancy)?;
    Ok(Json(BootstrapTenantResponse {
        tenant_id: tenant.id,
        owner_principal_id: owner.principal_id.to_string(),
        api_token: token.raw_token,
    }))
}

#[derive(Debug, Deserialize)]
struct IssueTokenRequest {
    principal_id: String,
    label: String,
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
struct IssueTokenResponse {
    token_id: Uuid,
    api_token: String,
    expires_at: Option<DateTime<Utc>>,
}

async fn issue_token(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<IssueTokenRequest>,
) -> ApiResult<Json<IssueTokenResponse>> {
    let tenant_id = parse_tenant(&tenant_id)?;
    let context = authenticate(&state, &headers, tenant_id).await?;
    if !context.can_manage_members() {
        return Err(ApiError::forbidden("membership management requires owner or admin"));
    }
    let token = state
        .auth
        .issue(
            tenant_id,
            PrincipalId::parse(request.principal_id).map_err(ApiError::bad_request)?,
            request.label,
            request.expires_at,
        )
        .await
        .map_err(ApiError::from_tenancy)?;
    Ok(Json(IssueTokenResponse {
        token_id: token.id,
        api_token: token.raw_token,
        expires_at: token.expires_at,
    }))
}

#[derive(Debug, Deserialize)]
struct SetQuotaRequest {
    dimension: QuotaDimension,
    hard_limit: u64,
}

async fn set_quota(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<SetQuotaRequest>,
) -> ApiResult<StatusCode> {
    let tenant_id = parse_tenant(&tenant_id)?;
    let context = authenticate(&state, &headers, tenant_id).await?;
    if !context.can_manage_billing() {
        return Err(ApiError::forbidden("quota management requires billing authority"));
    }
    state
        .tenant_repository
        .set_quota(&TenantQuota {
            tenant_id,
            dimension: request.dimension,
            hard_limit: request.hard_limit,
            updated_at: Utc::now(),
        })
        .await
        .map_err(ApiError::from_tenancy)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct CreateSessionRequest {
    title: String,
    idempotency_key: String,
}

async fn create_session(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateSessionRequest>,
) -> ApiResult<Json<Session>> {
    let tenant_id = parse_tenant(&tenant_id)?;
    let context = authenticate(&state, &headers, tenant_id).await?;
    require_runtime_mutation(&context)?;
    state
        .tenants
        .reserve(
            &context,
            QuotaDimension::Sessions,
            1,
            &request.idempotency_key,
        )
        .await
        .map_err(ApiError::from_tenancy)?;
    let runtime = state.runtimes.runtime(tenant_id).await.map_err(ApiError::internal)?;
    let session = runtime
        .control_plane()
        .create_session(request.title, &operation_context(&context))
        .await
        .map_err(ApiError::control_plane)?;
    state
        .tenant_repository
        .bind_resource(tenant_id, sessionweft_tenancy::ResourceKind::Session, &session.id.to_string())
        .await
        .map_err(ApiError::from_tenancy)?;
    Ok(Json(session))
}

async fn get_session(
    State(state): State<AppState>,
    Path((tenant_id, session_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<Session>> {
    let tenant_id = parse_tenant(&tenant_id)?;
    let _context = authenticate(&state, &headers, tenant_id).await?;
    let session_id = parse_session(&session_id)?;
    let runtime = state.runtimes.runtime(tenant_id).await.map_err(ApiError::internal)?;
    runtime
        .control_plane()
        .get_session(session_id)
        .await
        .map(Json)
        .map_err(ApiError::not_found)
}

async fn register_agent(
    State(state): State<AppState>,
    Path((tenant_id, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(manifest): Json<AgentManifest>,
) -> ApiResult<Json<AgentRecord>> {
    let tenant_id = parse_tenant(&tenant_id)?;
    let context = authenticate(&state, &headers, tenant_id).await?;
    require_runtime_mutation(&context)?;
    let runtime = state.runtimes.runtime(tenant_id).await.map_err(ApiError::internal)?;
    runtime
        .control_plane()
        .register_agent(parse_session(&session_id)?, manifest, &operation_context(&context))
        .await
        .map(Json)
        .map_err(ApiError::control_plane)
}

async fn create_workflow(
    State(state): State<AppState>,
    Path((tenant_id, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(definition): Json<WorkflowDefinition>,
) -> ApiResult<Json<WorkflowExecution>> {
    let tenant_id = parse_tenant(&tenant_id)?;
    let context = authenticate(&state, &headers, tenant_id).await?;
    require_runtime_mutation(&context)?;
    let runtime = state.runtimes.runtime(tenant_id).await.map_err(ApiError::internal)?;
    runtime
        .control_plane()
        .create_workflow(parse_session(&session_id)?, definition, &operation_context(&context))
        .await
        .map(Json)
        .map_err(ApiError::control_plane)
}

async fn acquire_lock(
    State(state): State<AppState>,
    Path((tenant_id, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(mut request): Json<LockRequest>,
) -> ApiResult<Json<LockLease>> {
    let tenant_id = parse_tenant(&tenant_id)?;
    let context = authenticate(&state, &headers, tenant_id).await?;
    require_runtime_mutation(&context)?;
    request.session_id = parse_session(&session_id)?;
    let runtime = state.runtimes.runtime(tenant_id).await.map_err(ApiError::internal)?;
    runtime
        .control_plane()
        .acquire_lock(&request, &operation_context(&context))
        .await
        .map(Json)
        .map_err(ApiError::control_plane)
}

async fn list_locks(
    State(state): State<AppState>,
    Path((tenant_id, session_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<LockLease>>> {
    let tenant_id = parse_tenant(&tenant_id)?;
    let _context = authenticate(&state, &headers, tenant_id).await?;
    let workspace_id = headers
        .get("x-sessionweft-workspace-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("default");
    let runtime = state.runtimes.runtime(tenant_id).await.map_err(ApiError::internal)?;
    runtime
        .control_plane()
        .list_locks(parse_session(&session_id)?, workspace_id)
        .await
        .map(Json)
        .map_err(ApiError::control_plane)
}

async fn upsert_plan(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(plan): Json<BillingPlan>,
) -> ApiResult<StatusCode> {
    let tenant_id = parse_tenant(&tenant_id)?;
    let context = authenticate(&state, &headers, tenant_id).await?;
    if !context.can_manage_billing() {
        return Err(ApiError::forbidden("plan management requires billing authority"));
    }
    let repository = PostgresBillingRepository::new(state.database.clone(), tenant_id);
    repository.upsert_plan(&plan).await.map_err(ApiError::billing)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct SubscribeRequest {
    plan_id: PlanId,
    period_start: DateTime<Utc>,
    period_end: DateTime<Utc>,
    idempotency_key: String,
}

async fn create_subscription(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<SubscribeRequest>,
) -> ApiResult<Json<sessionweft_billing::Subscription>> {
    let tenant_id = parse_tenant(&tenant_id)?;
    let context = authenticate(&state, &headers, tenant_id).await?;
    let provider = state.stripe.clone().ok_or_else(|| {
        ApiError::service_unavailable("Stripe billing adapter is not configured")
    })?;
    let repository = Arc::new(PostgresBillingRepository::new(state.database.clone(), tenant_id));
    BillingService::new(repository, provider)
        .subscribe(
            &context,
            &request.plan_id,
            request.period_start,
            request.period_end,
            &request.idempotency_key,
        )
        .await
        .map(Json)
        .map_err(ApiError::billing)
}

async fn get_entitlement(
    State(state): State<AppState>,
    Path((tenant_id, name)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let tenant_id = parse_tenant(&tenant_id)?;
    let _context = authenticate(&state, &headers, tenant_id).await?;
    let provider = state.stripe.clone().ok_or_else(|| {
        ApiError::service_unavailable("Stripe billing adapter is not configured")
    })?;
    let repository = Arc::new(PostgresBillingRepository::new(state.database.clone(), tenant_id));
    let value = BillingService::new(repository, provider)
        .entitlement(tenant_id, &name)
        .await
        .map_err(ApiError::billing)?;
    Ok(Json(json!({"name": name, "limit": value})))
}

#[derive(Debug, Deserialize)]
struct UsageRequest {
    meter: MeterName,
    quantity: u64,
    idempotency_key: String,
    occurred_at: DateTime<Utc>,
}

async fn record_usage(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UsageRequest>,
) -> ApiResult<Json<sessionweft_billing::UsageRecord>> {
    let tenant_id = parse_tenant(&tenant_id)?;
    let context = authenticate(&state, &headers, tenant_id).await?;
    let provider = state.stripe.clone().ok_or_else(|| {
        ApiError::service_unavailable("Stripe billing adapter is not configured")
    })?;
    let repository = Arc::new(PostgresBillingRepository::new(state.database.clone(), tenant_id));
    BillingService::new(repository, provider)
        .record_usage(
            &context,
            request.meter,
            request.quantity,
            &request.idempotency_key,
            request.occurred_at,
        )
        .await
        .map(Json)
        .map_err(ApiError::billing)
}

async fn stripe_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let provider = state.stripe.clone().ok_or_else(|| {
        ApiError::service_unavailable("Stripe billing adapter is not configured")
    })?;
    let signature = headers
        .get("stripe-signature")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("Stripe-Signature header is missing"))?;
    let event = provider
        .verify_webhook(&body, signature, Utc::now())
        .map_err(ApiError::billing)?;
    let repository = PostgresBillingRepository::new(state.database.clone(), event.tenant_id);
    repository
        .apply_webhook(&event)
        .await
        .map_err(ApiError::billing)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    expected_tenant: TenantId,
) -> ApiResult<TenantContext> {
    let raw_token = bearer(headers)?;
    let resolved = state
        .auth
        .resolve(raw_token)
        .await
        .map_err(ApiError::from_tenancy)?
        .ok_or_else(|| ApiError::unauthorized("invalid or expired tenant token"))?;
    if resolved.tenant_id != expected_tenant {
        return Err(ApiError::not_found_message("tenant resource was not found"));
    }
    state
        .tenants
        .context(
            resolved.tenant_id,
            &resolved.principal_id,
            Uuid::new_v4(),
        )
        .await
        .map_err(ApiError::from_tenancy)
}

fn operation_context(context: &TenantContext) -> OperationContext {
    OperationContext::new(
        context.correlation_id,
        Some(format!("tenant:{}:{}", context.tenant_id, context.principal_id)),
    )
}

fn require_runtime_mutation(context: &TenantContext) -> ApiResult<()> {
    if context.can_mutate_runtime() {
        Ok(())
    } else {
        Err(ApiError::forbidden("tenant role cannot mutate Runtime state"))
    }
}

fn require_bootstrap(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    let provided = headers
        .get("x-sessionweft-bootstrap-token")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("bootstrap token is missing"))?;
    let provided_hash: [u8; 32] = Sha256::digest(provided.as_bytes()).into();
    if provided_hash.ct_eq(&state.bootstrap_hash).into() {
        Ok(())
    } else {
        Err(ApiError::unauthorized("bootstrap token is invalid"))
    }
}

fn bearer(headers: &HeaderMap) -> ApiResult<&str> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("Authorization header is missing"))?;
    value
        .strip_prefix("Bearer ")
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| ApiError::unauthorized("Authorization must use Bearer scheme"))
}

fn parse_tenant(value: &str) -> ApiResult<TenantId> {
    value
        .parse()
        .map_err(|_| ApiError::bad_request("tenant ID must be a UUID"))
}

fn parse_session(value: &str) -> ApiResult<SessionId> {
    value
        .parse()
        .map_err(|_| ApiError::bad_request("session ID must be a UUID"))
}

fn default_quotas() -> [(QuotaDimension, u64); 8] {
    [
        (QuotaDimension::Sessions, 100),
        (QuotaDimension::ActiveAgents, 50),
        (QuotaDimension::QueuedTasks, 10_000),
        (QuotaDimension::IndexedFiles, 10_000),
        (QuotaDimension::EventBacklog, 1_000_000),
        (QuotaDimension::ProviderTokens, 10_000_000),
        (QuotaDimension::ToolInvocations, 100_000),
        (QuotaDimension::StorageBytes, 10 * 1024 * 1024 * 1024),
    ]
}

fn provider_registry() -> Result<ProviderRegistry> {
    let mut registry = ProviderRegistry::new();
    registry.register(EchoProvider);
    if let Ok(base_url) = env::var("SESSIONWEFT_OLLAMA_URL") {
        registry.register(OllamaProvider::new(base_url, Duration::from_secs(120))?);
    }
    Ok(registry)
}

fn stripe_provider() -> Result<Option<Arc<StripeBillingProvider>>> {
    let Ok(secret_key) = env::var("SESSIONWEFT_STRIPE_SECRET_KEY") else {
        return Ok(None);
    };
    let webhook_secret = required_env("SESSIONWEFT_STRIPE_WEBHOOK_SECRET")?;
    let price_ids = parse_plan_map(env::var("SESSIONWEFT_STRIPE_PRICE_IDS_JSON").unwrap_or_else(|_| "{}".into()))?;
    let meter_event_names = parse_meter_map(
        env::var("SESSIONWEFT_STRIPE_METER_EVENTS_JSON").unwrap_or_else(|_| "{}".into()),
    )?;
    let provider = StripeBillingProvider::new(StripeBillingConfig {
        secret_key,
        webhook_secret,
        api_base: env::var("SESSIONWEFT_STRIPE_API_BASE")
            .unwrap_or_else(|_| "https://api.stripe.com".into()),
        price_ids,
        meter_event_names,
        request_timeout: Duration::from_secs(30),
        signature_tolerance: Duration::from_secs(300),
    })?;
    Ok(Some(Arc::new(provider)))
}

fn parse_plan_map(raw: String) -> Result<BTreeMap<PlanId, String>> {
    serde_json::from_str::<BTreeMap<String, String>>(&raw)?
        .into_iter()
        .map(|(key, value)| Ok((PlanId::parse(key)?, value)))
        .collect::<Result<_, sessionweft_billing::BillingError>>()
        .map_err(Into::into)
}

fn parse_meter_map(raw: String) -> Result<BTreeMap<MeterName, String>> {
    serde_json::from_str::<BTreeMap<String, String>>(&raw)?
        .into_iter()
        .map(|(key, value)| Ok((MeterName::parse(key)?, value)))
        .collect::<Result<_, sessionweft_billing::BillingError>>()
        .map_err(Into::into)
}

fn required_env(name: &str) -> Result<String> {
    let value = env::var(name).with_context(|| format!("{name} is required"))?;
    if value.trim().is_empty() {
        anyhow::bail!("{name} cannot be empty");
    }
    Ok(value)
}

type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn bad_request(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", error.to_string())
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    fn not_found(error: impl std::fmt::Display) -> Self {
        Self::not_found_message(error.to_string())
    }

    fn not_found_message(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    fn service_unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "service_unavailable", message)
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(error = %error, "SaaS Runtime internal error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "internal Runtime failure",
        )
    }

    fn from_tenancy(error: sessionweft_tenancy::TenancyError) -> Self {
        match error {
            sessionweft_tenancy::TenancyError::AccessDenied { .. } => {
                Self::forbidden("tenant access denied")
            }
            sessionweft_tenancy::TenancyError::TenantNotFound(_)
            | sessionweft_tenancy::TenancyError::ResourceNotFound { .. } => {
                Self::not_found_message("tenant resource was not found")
            }
            sessionweft_tenancy::TenancyError::QuotaExceeded { .. } => {
                Self::new(StatusCode::TOO_MANY_REQUESTS, "quota_exceeded", error.to_string())
            }
            sessionweft_tenancy::TenancyError::Validation(_) => Self::bad_request(error),
            _ => Self::internal(error),
        }
    }

    fn billing(error: sessionweft_billing::BillingError) -> Self {
        match error {
            sessionweft_billing::BillingError::AccessDenied => {
                Self::forbidden("billing access denied")
            }
            sessionweft_billing::BillingError::Validation(_) => Self::bad_request(error),
            sessionweft_billing::BillingError::NoActiveSubscription(_) => {
                Self::new(StatusCode::PAYMENT_REQUIRED, "subscription_required", error.to_string())
            }
            sessionweft_billing::BillingError::Provider(_)
            | sessionweft_billing::BillingError::ProviderUncertain(_) => {
                Self::service_unavailable(error.to_string())
            }
            _ => Self::internal(error),
        }
    }

    fn control_plane(error: impl std::fmt::Display) -> Self {
        Self::bad_request(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "error": {
                    "code": self.code,
                    "message": self.message,
                }
            })),
        )
            .into_response()
    }
}
