# RFC-0001: Runtime Vertical Slice

- Status: Accepted for implementation
- Date: 2026-07-22
- ADRs: ADR-0001 through ADR-0004

## 1. Scope

This RFC defines the first executable SessionWeft slice. It proves the core ownership and recovery model before workflow, locking, workspace indexing, memory, MCP and IDE implementation.

Included:

- Session aggregate and commands;
- optimistic concurrency;
- SQLite WAL repository;
- transactional outbox;
- bounded local event transport;
- provider contract;
- Echo and Ollama-compatible providers;
- Runtime use-case service;
- versioned HTTP/JSON control API for the slice;
- CLI client;
- bearer-token middleware for non-local service mode;
- structured telemetry and redaction;
- tests and CI.

The permanent public API remains gRPC/Protobuf. The HTTP control adapter is an implementation bootstrap and is kept outside domain crates so it can be replaced without changing Runtime contracts.

## 2. Domain contracts

### Session

```text
Session {
  id: UUID,
  version: u64,
  status: Active | Archived,
  title: string,
  messages: Message[],
  provider: ProviderSelection?,
  created_at,
  updated_at
}
```

### Commands

- `CreateSession(title)`
- `AppendMessage(expected_version, role, content)`
- `SelectProvider(expected_version, provider, model)`
- `ArchiveSession(expected_version)`
- `RunProvider(expected_version, input)`

Every successful mutation increments the version exactly once.

### Errors

- `not_found`
- `conflict`
- `validation`
- `unauthorized`
- `forbidden`
- `provider_unavailable`
- `provider_rate_limited`
- `provider_invalid_response`
- `storage`
- `internal`

Errors have stable machine-readable codes and a correlation ID. Internal causes and secrets are not exposed to clients.

## 3. Persistence schema

```sql
CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  version INTEGER NOT NULL,
  status TEXT NOT NULL,
  title TEXT NOT NULL,
  data_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE outbox (
  event_id TEXT PRIMARY KEY,
  session_id TEXT,
  event_type TEXT NOT NULL,
  schema_version INTEGER NOT NULL,
  payload_json TEXT NOT NULL,
  correlation_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  published_at TEXT,
  publish_attempts INTEGER NOT NULL DEFAULT 0,
  last_error TEXT
);
```

SQLite setup requires `PRAGMA journal_mode=WAL`, `foreign_keys=ON` and a bounded busy timeout.

## 4. Repository contract

```text
create(session, events) -> committed session
get(session_id) -> session?
list(limit, cursor) -> sessions
save(expected_version, next_session, events) -> committed session | conflict
pending_outbox(limit) -> events
mark_outbox_published(event_id)
mark_outbox_failed(event_id, sanitized_error)
```

The repository transaction owns compare-and-swap and outbox insertion.

## 5. Provider contract

```text
Provider::name() -> static name
Provider::capabilities() -> ProviderCapabilities
Provider::complete(ProviderRequest, CancellationToken) -> ProviderResponse
```

The first slice uses complete-response calls. The domain response already uses typed content and usage fields so streaming can be added without changing Session ownership.

`ProviderRequest` contains Runtime-owned messages, selected model and optional tools. `ProviderResponse` contains text, tool requests, usage and optional provider request ID.

## 6. Runtime service

The Runtime service:

1. validates input;
2. loads the Session;
3. verifies expected version;
4. checks lifecycle and policy;
5. calls a provider when required;
6. creates the next aggregate and domain events;
7. persists them atomically;
8. returns the committed aggregate.

Provider input is committed before the provider call so a provider failure never erases user intent. The assistant output is a separate versioned mutation.

## 7. API

Bootstrap endpoints:

- `GET /health/live`
- `GET /health/ready`
- `POST /v1/sessions`
- `GET /v1/sessions/{id}`
- `GET /v1/sessions`
- `POST /v1/sessions/{id}/messages`
- `POST /v1/sessions/{id}/provider`
- `POST /v1/sessions/{id}/run`
- `POST /v1/sessions/{id}/archive`

Mutation requests include `expected_version` except create. Responses include the committed version and correlation ID.

## 8. Authentication

- Loopback-only local mode may run without a token.
- Any non-loopback bind requires `SESSIONWEFT_API_TOKEN`.
- When configured, requests except liveness require `Authorization: Bearer ...`.
- Tokens are compared without logging either value.

## 9. Outbox worker

- Poll unpublished rows in bounded batches.
- Publish to the configured transport.
- Mark success after publication.
- Record sanitized error and increment attempts after failure.
- Use bounded exponential backoff.
- Shutdown cooperatively.

## 10. Testing

Mandatory tests:

- create/reload survives repository reopen;
- stale version returns conflict;
- state and outbox commit together;
- failed provider leaves committed user input;
- provider switch preserves Session ID;
- duplicate event IDs are rejected/deduplicated;
- unauthorized API request is rejected;
- redaction removes configured secret fixtures;
- outbox resumes after Runtime restart.

## 11. Future-compatible boundaries

Traits are reserved for:

- `EventTransport` JetStream adapter;
- PostgreSQL repository;
- gRPC API;
- workflow engine;
- lock manager;
- workspace and Git services;
- memory/vector projections;
- MCP policy gate.
