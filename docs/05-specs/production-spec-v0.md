# Production Specification v0

Status: **Approved for vertical-slice implementation**  
Date: 2026-07-22

This specification is intentionally narrower than GA. It defines measurable gates for the first Runtime slice.

## Reliability

- A committed Session mutation survives a forced process restart.
- A failed event publication does not roll back committed Session state.
- Unpublished outbox rows resume after restart.
- Stale expected versions return conflict and never overwrite the latest aggregate.
- Provider timeout or invalid response leaves committed user input intact.

## Availability

- Local mode has no formal uptime SLA.
- Readiness is false until migrations, repository and provider registry initialization succeed.
- Liveness does not depend on external providers.
- The service shuts down cooperatively and stops accepting new work before closing workers.

## Data durability

- SQLite WAL is used only on a local filesystem.
- Database, WAL and SHM files are treated as one persistence unit during backup.
- Service-mode PostgreSQL backup/restore is required before declaring beta.
- Search and vector projections must be rebuildable.

## Security

- Local default bind: `127.0.0.1`.
- Non-loopback bind without API token: startup failure.
- Secret values are never written to normal logs, outbox payloads or memory records.
- Request bodies are not logged by default.
- Tool/plugin execution is not part of this slice and remains default-deny.

## Observability

Required structured fields where applicable:

- `correlation_id`
- `session_id`
- `session_version`
- `operation`
- `provider`
- `event_id`
- `event_type`
- `outcome`
- `latency_ms`

Required counters/histograms:

- Session command total and latency
- optimistic conflict total
- storage error total
- outbox pending count and oldest age
- outbox publish success/failure total
- provider request total, latency and error class
- authorization denial total

Session ID and event ID must not be metric labels.

## Performance test baselines

The first CI baseline is functional rather than a release SLO:

- 1,000 sequential Session mutations complete without data loss.
- 50 concurrent stale-version attempts produce deterministic conflicts.
- 1,000 outbox events are published without unbounded memory growth.
- API request and response bodies are bounded by configuration.

Numeric latency SLOs are recorded after benchmark evidence on declared hardware.

## Compatibility

- Persisted aggregate includes a schema version.
- Event envelope includes a schema version.
- Unknown future fields are ignored where safe.
- Destructive migrations require backup and rollback instructions.
- CLI supports machine-readable JSON output.

## Release gate

The vertical slice can merge when:

1. formatting, lint and tests pass;
2. migrations are included;
3. recovery and concurrency tests pass;
4. API authentication tests pass;
5. README includes local run instructions;
6. dependency lockfile is committed;
7. no known critical/high dependency vulnerability is accepted without a documented exception.
