from pathlib import Path

path = Path("apps/sessionweftd/src/main.rs")
text = path.read_text()

if not text.startswith("mod control_plane_api;"):
    text = "mod control_plane_api;\n\n" + text

old_import = '''use sessionweft_core::{DomainError, MessageRole, Session, SessionId};
use sessionweft_provider::{EchoProvider, OllamaProvider, ProviderError, ProviderRegistry};
'''
new_import = '''use sessionweft_control_plane::RuntimeControlPlane;
use sessionweft_core::{DomainError, MessageRole, Session, SessionId};
use sessionweft_execution_sqlite::SqliteAgentRepository;
use sessionweft_knowledge_sqlite::SqliteMemoryRepository;
use sessionweft_orchestration_sqlite::SqliteOrchestrationRepository;
use sessionweft_provider::{EchoProvider, OllamaProvider, ProviderError, ProviderRegistry};
'''
if old_import in text:
    text = text.replace(old_import, new_import, 1)
elif "use sessionweft_control_plane::RuntimeControlPlane;" not in text:
    raise SystemExit("daemon import marker not found")

old_state = '''#[derive(Clone)]
struct AppState {
    runtime: RuntimeService<SqliteSessionRepository>,
    providers: Arc<ProviderRegistry>,
    api_token: Option<Arc<str>>,
}
'''
new_state = '''type LocalControlPlane = RuntimeControlPlane<
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
}
'''
if old_state in text:
    text = text.replace(old_state, new_state, 1)
elif "control_plane: Arc<LocalControlPlane>" not in text:
    raise SystemExit("AppState marker not found")

old_runtime = '''    let providers = Arc::new(providers);
    let runtime = RuntimeService::new(Arc::clone(&repository), Arc::clone(&providers));

    let transport = Arc::new(LocalEventTransport::new(1_024));
'''
new_runtime = '''    let providers = Arc::new(providers);
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

    let transport = Arc::new(LocalEventTransport::new(1_024));
'''
if old_runtime in text:
    text = text.replace(old_runtime, new_runtime, 1)
elif "failed to initialize Agent repository" not in text:
    raise SystemExit("Runtime initialization marker not found")

old_state_value = '''    let state = AppState {
        runtime,
        providers,
        api_token,
    };
'''
new_state_value = '''    let state = AppState {
        runtime,
        control_plane,
        providers,
        api_token,
    };
'''
if old_state_value in text:
    text = text.replace(old_state_value, new_state_value, 1)
elif "        control_plane," not in text:
    raise SystemExit("AppState construction marker not found")

old_routes = '''        .route("/v1/sessions/{id}/archive", post(archive_session))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
'''
new_routes = '''        .route("/v1/sessions/{id}/archive", post(archive_session))
        .merge(control_plane_api::routes())
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
'''
if old_routes in text:
    text = text.replace(old_routes, new_routes, 1)
elif ".merge(control_plane_api::routes())" not in text:
    raise SystemExit("router marker not found")

path.write_text(text)
