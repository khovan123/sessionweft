# ADR-0005: Persisted Runtime-Owned Workflow DAG

- Status: Accepted
- Date: 2026-07-22
- Issue: #5

## Context

SessionWeft requires retries, fan-out/fan-in, approvals, fallback and restart-safe execution. The first production slice must preserve Runtime-owned state without adding a mandatory external orchestration service.

## Decision

1. Workflow definitions are versioned DAGs owned by the Runtime.
2. Definitions are validated for duplicate IDs, missing dependencies and cycles before execution.
3. Execution state is persisted independently but referenced by Session ID.
4. Every command uses optimistic execution versions.
5. Node transitions and their outbox events commit in one transaction.
6. Ready nodes are derived from persisted dependency state.
7. Approval nodes persist a waiting state and require an explicit decision.
8. Retry count, fallback activation and sanitized failure information are persisted.
9. Temporal remains a future adapter/replacement candidate rather than a mandatory dependency.

## Consequences

- The first scheduler remains small and auditable.
- Long-running timer and distributed scheduling features require later extension.
- Definition upgrades for active executions need a separate compatibility RFC.
- Side-effecting task handlers still require idempotency keys or compensation.

## Alternatives

- Temporal as a mandatory first dependency: deferred because it adds service and SDK coupling before the Runtime contracts are stable.
- In-memory workflow state: rejected because restart and handover would lose progress.
- Provider-owned agent planning state: rejected because it violates Session-first ownership.
