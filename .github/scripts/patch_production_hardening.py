from pathlib import Path

cargo = Path("apps/sessionweftd/Cargo.toml")
text = cargo.read_text()
dependency = 'sessionweft-observability = { path = "../../crates/sessionweft-observability" }\n'
if dependency not in text:
    marker = 'sessionweft-orchestration-sqlite = { path = "../../crates/sessionweft-orchestration-sqlite" }\n'
    if marker not in text:
        raise SystemExit("daemon Cargo dependency marker not found")
    text = text.replace(marker, marker + dependency, 1)
cargo.write_text(text)

main = Path("apps/sessionweftd/src/main.rs")
text = main.read_text()
replacements = [
    (
        'use std::{collections::BTreeSet, env, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};',
        'use std::{\n    collections::BTreeSet,\n    env,\n    net::SocketAddr,\n    path::PathBuf,\n    sync::Arc,\n    time::{Duration, Instant},\n};',
    ),
    (
        'use sessionweft_orchestration_sqlite::SqliteOrchestrationRepository;\n',
        'use sessionweft_observability::MetricsRegistry;\nuse sessionweft_orchestration_sqlite::SqliteOrchestrationRepository;\n',
    ),
    (
        '    event_journal: Arc<SqliteClientEventJournal>,\n    pty: Arc<PtySupervisor>,',
        '    event_journal: Arc<SqliteClientEventJournal>,\n    pty: Arc<PtySupervisor>,\n    metrics: Arc<MetricsRegistry>,',
    ),
    (
        '    let state = AppState {\n        runtime,',
        '    let metrics = Arc::new(MetricsRegistry::new());\n    let state = AppState {\n        runtime,',
    ),
    (
        '        event_journal,\n        pty,\n    };',
        '        event_journal,\n        pty,\n        metrics,\n    };',
    ),
    (
        '        .route("/health/ready", get(readiness))\n',
        '        .route("/health/ready", get(readiness))\n        .route("/metrics", get(metrics))\n',
    ),
    (
        '        .merge(protected)\n        .layer(TraceLayer::new_for_http())\n        .with_state(state);',
        '        .merge(protected)\n        .layer(TraceLayer::new_for_http())\n        .layer(middleware::from_fn_with_state(\n            state.clone(),\n            observe_requests,\n        ))\n        .with_state(state);',
    ),
    (
        '    if !authorized {\n        let correlation_id = Uuid::new_v4();',
        '    if !authorized {\n        state.metrics.record_auth_denied();\n        let correlation_id = Uuid::new_v4();',
    ),
    (
        '    if method != Method::GET && response.status().is_success() {\n        let event = EventEnvelope::new(',
        '    if method != Method::GET && response.status().is_success() {\n        state.metrics.record_successful_mutation();\n        let event = EventEnvelope::new(',
    ),
    (
        '        if let Err(error) = state.event_journal.append(&event).await {\n            warn!',
        '        if let Err(error) = state.event_journal.append(&event).await {\n            state.metrics.record_event_journal_failure();\n            warn!',
    ),
]
for old, new in replacements:
    if old in text:
        text = text.replace(old, new, 1)
    elif new not in text:
        raise SystemExit(f"main.rs patch marker not found: {old[:80]}")

handler_marker = '''async fn authenticate(State(state): State<AppState>, request: Request, next: Next) -> Response {'''
handler = '''async fn observe_requests(\n    State(state): State<AppState>,\n    request: Request,\n    next: Next,\n) -> Response {\n    let method = request.method().as_str().to_owned();\n    let started = Instant::now();\n    let response = next.run(request).await;\n    state\n        .metrics\n        .record_http(&method, response.status().as_u16(), started.elapsed());\n    response\n}\n\nasync fn metrics(State(state): State<AppState>) -> impl IntoResponse {\n    (\n        [(\n            header::CONTENT_TYPE,\n            "text/plain; version=0.0.4; charset=utf-8",\n        )],\n        state.metrics.render_prometheus(),\n    )\n}\n\n'''
if handler not in text:
    if handler_marker not in text:
        raise SystemExit("authenticate handler marker not found")
    text = text.replace(handler_marker, handler + handler_marker, 1)
main.write_text(text)
