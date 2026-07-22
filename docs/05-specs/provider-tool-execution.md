# Provider and Tool Execution Ledger

Status: implementation and verification slice following scheduler prerequisite enforcement.

## Purpose

Provider and Tool actions execute only for an active scheduler Claim that has passed capability, approval and lock-fence validation. External side effects are tracked in a durable ledger so worker restarts do not cause blind duplicate calls.

## Execution states

- `prepared`: authorization and prerequisites were validated, but no external action has started.
- `running`: the worker persisted intent immediately before invoking the Provider or Tool.
- `succeeded`: the external action returned a persisted result.
- `failed`: the external action returned a persisted sanitized error.
- `uncertain`: a running record exceeded the recovery threshold and must be reconciled instead of retried automatically.

## Invariants

1. A Claim and idempotency key each map to at most one execution record.
2. The Claim must be active and owned by a running, non-stale Agent.
3. Provider actions require the Provider capability.
4. Tool actions require all descriptor permissions; high and critical risk actions require a valid Claim-scoped approval.
5. A persisted lock fence is revalidated against the current lease immediately before preparation.
6. The worker persists `running` before invoking an adapter.
7. A stale `running` record becomes `uncertain`; it is never automatically invoked again.
8. A persisted terminal result is finalized into scheduler Claim completion or failure without repeating the external action.

## Adapters

Provider actions use `ProviderRegistry`. Tool actions use `ToolRegistry` and the `ToolAction` contract. The deterministic Echo adapter is intended for tests and smoke verification; MCP-backed isolated tools are introduced in issue #32.

## Worker lifecycle

The execution worker repeatedly:

1. marks stale running records uncertain;
2. discovers active Claims with execution specs;
3. prepares eligible records;
4. invokes prepared actions;
5. persists success or failure;
6. finalizes terminal results into scheduler Claim state;
7. backs off while idle and shuts down gracefully.
