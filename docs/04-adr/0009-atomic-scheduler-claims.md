# ADR-0009: Atomic Scheduler Claims

- Status: Accepted for local SQLite mode
- Date: 2026-07-22
- Scope: Durable scheduler claim authority

## Context

Workflow node ownership and Agent task ownership were previously persisted by separate repositories. Calling those repositories sequentially can leave partial state when Runtime crashes between writes and can allow competing workers to observe inconsistent ownership.

## Decision

Local mode uses a dedicated scheduler adapter that performs the following in one SQLite transaction:

1. load the Scheduler Plan, Workflow and Agent;
2. validate Session, Agent availability and capability requirements;
3. select one ready Workflow node;
4. transition the node to running;
5. assign the corresponding task to the Agent;
6. persist a unique active Task Claim and deterministic idempotency key;
7. update Workflow and Agent versions;
8. append all Workflow, Agent and Scheduler events to the transactional outbox.

A partial unique index permits at most one active claim for a Workflow node. Local claim acquisition is additionally serialized inside one Runtime process. PostgreSQL service mode must implement equivalent transaction and row-lock semantics; it must not rely on the process-local mutex.

## Consequences

- Runtime restart can reconstruct active ownership from persisted claims.
- Completion and failure can be idempotent at the claim boundary.
- Scheduler storage knows the persisted Workflow and Agent table contracts, so compatibility tests are mandatory when those schemas change.
- Claim recovery and stale-Agent handover remain a following scheduler slice.

## Rejected alternatives

- Sequential WorkflowRepo and AgentRepo writes: rejected because they are not atomic.
- In-memory task ownership: rejected because it cannot recover after restart.
- Eventual reconciliation without a claim authority: rejected because duplicate external side effects remain possible.
