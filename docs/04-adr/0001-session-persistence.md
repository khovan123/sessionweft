# ADR-0001: Versioned Session Aggregate Persistence

- Status: Accepted
- Date: 2026-07-22
- Issues: #8, #10

## Context

SessionWeft requires Session to remain the single source of truth across process restarts, client disconnects, provider changes and agent handover. Local and service deployments need different databases without different domain semantics.

## Decision

1. Use a Runtime-generated UUID as Session identity.
2. Persist Session as a versioned aggregate.
3. Require every mutation to provide the expected version.
4. Apply updates with compare-and-swap semantics.
5. Store domain-event outbox rows in the same transaction.
6. Use SQLite WAL for local mode and PostgreSQL for service mode.
7. Keep database implementations behind a `SessionRepository` contract.
8. Treat search, memory and vector indexes as rebuildable projections.

## Consequences

- Concurrent writes fail explicitly instead of overwriting each other.
- SQLite's single-writer behavior limits local write throughput but is acceptable for the local deployment scope.
- PostgreSQL transactions may still require bounded retry under serialization or optimistic conflicts.
- Aggregate size must be monitored; high-volume records may move to referenced tables without changing ownership rules.

## Alternatives

- Provider-managed conversation state: rejected because provider switching would change authority.
- Pure event sourcing for every domain object: deferred because it adds operational and migration complexity before the first slice.
- Separate state update and event publication transactions: rejected because it creates unrecoverable gaps.

## Recovery

On startup, migrations run, the database is opened, and unpublished outbox rows are resumed. A committed Session version is never rolled back because a later event publication failed.
