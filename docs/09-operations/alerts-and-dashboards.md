# Alerts and Dashboards

## Metric contract

Runtime exposes bounded Prometheus metrics. Labels are limited to HTTP method and status; Session, Agent, Workflow, path and correlation IDs are intentionally excluded.

Core metrics:

- `sessionweft_runtime_up`
- `sessionweft_process_start_time_seconds`
- `sessionweft_http_requests_total{method,status}`
- `sessionweft_http_request_duration_seconds_bucket{method,le}`
- `sessionweft_http_request_duration_seconds_sum{method}`
- `sessionweft_http_request_duration_seconds_count{method}`
- `sessionweft_auth_denied_total`
- `sessionweft_successful_mutations_total`
- `sessionweft_event_journal_failures_total`

The dashboard is stored at `deploy/observability/sessionweft-dashboard.json`; alert rules are stored at `deploy/observability/prometheus-rules.yml`.

## Required alerts

| Alert | Severity | Initial response |
|---|---|---|
| Runtime down for two minutes | Critical | start incident response; verify process/database availability |
| HTTP 5xx rate above 5% for ten minutes | Critical | identify failing route and correlated dependency |
| Read p99 above two seconds | Warning | inspect PostgreSQL, workspace retrieval and client reconnect load |
| Mutation p99 above four seconds | Warning | inspect lock contention, Outbox and database transactions |
| Any event-journal append failure | Critical | stop claiming durable client delivery until storage is healthy |
| Authentication denials above one per second | Warning | investigate credential/configuration error or abuse |
| PostgreSQL exporter down | Critical | follow backup/restore and database incident procedure |
| NATS monitoring down | Critical | preserve PostgreSQL Outbox and restore transport connectivity |

## Dashboard review

During normal operation, review:

1. Runtime availability and restart time.
2. Request rate split by method/status.
3. Read and mutation p99 latency against SLO.
4. Authentication denials and durable journal failures.
5. PostgreSQL and NATS availability from dependency dashboards.

During an incident, narrow the time range to include five minutes before the first alert. Use structured logs and correlation IDs for high-cardinality detail rather than adding unbounded metric labels.

## Alert quality rules

- Every alert has a runbook link and owner.
- Critical alerts indicate data safety, security or complete availability risk.
- Warning alerts must be actionable and should not page without a sustained condition.
- Alert changes require a test with `promtool check rules` in the hardening workflow.
- Dashboard JSON must parse and all PromQL expressions must refer to emitted or documented exporter metrics.
