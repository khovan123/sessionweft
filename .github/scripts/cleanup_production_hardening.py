from pathlib import Path
import re

path = Path("apps/sessionweftd/src/main.rs")
text = path.read_text()

text = re.sub(
    r"(?:use sessionweft_observability::MetricsRegistry;\n)+",
    "use sessionweft_observability::MetricsRegistry;\n",
    text,
)
text = re.sub(
    r"(?:    metrics: Arc<MetricsRegistry>,\n)+",
    "    metrics: Arc<MetricsRegistry>,\n",
    text,
)
text = re.sub(
    r"(?:    let metrics = Arc::new\(MetricsRegistry::new\(\)\);\n)+",
    "    let metrics = Arc::new(MetricsRegistry::new());\n",
    text,
)
text = re.sub(
    r'(?:        \.route\("/metrics", get\(metrics\)\)\n)+',
    '        .route("/metrics", get(metrics))\n',
    text,
)

start = text.find("async fn observe_requests(")
end = text.find("async fn authenticate(")
if start == -1 or end == -1 or end <= start:
    raise SystemExit("metrics handler boundaries not found")
canonical = '''async fn observe_requests(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
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

'''
text = text[:start] + canonical + text[end:]

for call in (
    "state.metrics.record_auth_denied();",
    "state.metrics.record_successful_mutation();",
    "state.metrics.record_event_journal_failure();",
):
    escaped = re.escape(call)
    text = re.sub(rf"(?:\s*{escaped})+", f"\n        {call}", text)

path.write_text(text)
