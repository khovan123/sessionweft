# Resumable Client Protocol

Status: production baseline for issue #35.

## Ownership

CLI, TUI and VS Code are stateless adapters to Runtime. They do not open SessionWeft databases, call providers directly, own workflow execution or terminate Runtime work when a client closes.

Runtime owns:

- Session, Agent, Workflow, Lock and approval state;
- the durable event journal and cursor allocation;
- PTY processes, output retention, resize and cancellation;
- authentication and command audit events.

## Versioning and errors

Every new client envelope includes `protocol_version` and `correlation_id`. The baseline protocol version is `1`. A client rejects an unsupported protocol version rather than interpreting unknown fields as authority.

Error responses include:

- stable `code`;
- human-readable `message`;
- `correlation_id`;
- `retryable`;
- `committed_version` when an external failure happened after Runtime committed durable state.

Legacy CLI commands retain their names and JSON behavior. The `client` command group exposes the versioned protocol, resumable events, aggregate resource view and PTY lifecycle.

## Event cursor and resume

Runtime writes every Outbox event to `client_event_journal` before the downstream local transport is considered published. The journal assigns a monotonically increasing cursor and deduplicates by event ID.

Clients request events after a cursor:

```text
GET /v1/events?after=<cursor>&limit=<limit>
```

The response reports `next`, `latest`, `has_more` and revision-preserving event envelopes. SSE uses the same cursor contract at `/v1/events/stream`. Reopening the SQLite journal after daemon restart preserves cursor ordering and duplicate-event idempotency.

## Aggregate resource view

```text
GET /v1/sessions/{session_id}/client-view
```

Optional query parameters select an Agent, Workflow and workspace lock set. The response includes:

- the durable Session;
- optional Agent and Workflow snapshots;
- active locks for the selected workspace;
- approval nodes currently waiting for a decision;
- generation time and protocol version.

Approval decisions continue to use the existing expected-version workflow endpoint. A stale client therefore receives a version conflict instead of overwriting a newer decision.

## Runtime-owned PTY

The PTY supervisor accepts only allowlisted executables, canonical working directories under the configured workspace root and allowlisted environment variables. It clears the inherited environment.

Runtime provides:

- create;
- descriptor read;
- bounded input;
- resize;
- output polling by cursor;
- explicit cancellation.

Output is retained in a bounded buffer. Old chunks are evicted and later chunks carry `truncated=true`. Dropping an HTTP request, CLI, TUI or VS Code window does not cancel the PTY. Only the explicit cancellation endpoint sends a kill request.

## Authentication and credentials

All client routes are inside the existing bearer-authenticated router. Non-loopback daemon binds require `SESSIONWEFT_API_TOKEN`.

Credential boundaries:

- CLI and TUI read a token from process arguments or `SESSIONWEFT_API_TOKEN` and do not persist it;
- VS Code stores the bearer token only in `ExtensionContext.secrets`;
- VS Code stores the non-secret Session ID and event cursor in workspace state;
- event payloads, PTY output and tokens are never written to extension configuration.

## Client behavior

### CLI

`sessionweft client` supports protocol inspection, event follow, client view and PTY create/input/resize/output/cancel. Event follow emits one JSON object per line.

### TUI

The Ratatui client polls the aggregate resource view and event cursor. It displays Session, optional Agent/Workflow, locks, events and approvals. Closing the TUI stops polling only.

### VS Code

The extension uses the browser-safe HTTP API. It provides attach, secure token configuration, refresh, approve and reject commands. Extension disposal aborts active requests and timers only; Runtime work remains active.

## Reconnect and offline handling

A client persists or retains only its latest acknowledged cursor. On reconnect it requests events after that cursor and refreshes the aggregate view. Daemon restarts, transient network failures and offline states are surfaced without clearing the cursor or issuing cancellation.

## Limits

- event batch: 1–1000 records;
- PTY rows/columns: 1–1000;
- PTY input: 1 MiB per request;
- retained PTY output: maximum 16 MiB per PTY;
- SSE keepalive: 15 seconds;
- long-poll wait: maximum 30 seconds.
