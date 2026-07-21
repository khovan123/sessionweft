# RFC-0002: Workflow Orchestration and Hierarchical Locking

- Status: Accepted for implementation
- Date: 2026-07-22
- ADRs: ADR-0005, ADR-0006

## Scope

This RFC adds durable workflow execution and workspace lock authority to the existing Session Runtime.

Included:

- versioned workflow definitions and executions;
- DAG validation and cycle rejection;
- fan-out/fan-in readiness;
- bounded retry and fallback activation;
- persisted approval decisions;
- SQLite execution repository and outbox integration;
- Workspace/Directory/File/Symbol lock resources;
- shared/exclusive compatibility;
- lease heartbeat, expiry and fencing-token validation;
- local-mode atomic audit events;
- domain and persistence tests.

Deferred:

- distributed PostgreSQL lock adapter;
- fair wait queue;
- workflow timers and cron triggers;
- automatic compensation execution;
- active workflow definition migration;
- merge queue integration;
- HTTP/gRPC client endpoints, which follow after domain and storage contracts are stable.

## Workflow state

Execution statuses:

- running;
- succeeded;
- failed;
- cancelled.

Node statuses:

- pending;
- ready;
- running;
- waiting approval;
- succeeded;
- failed;
- skipped.

Every mutation requires `expected_version`. A stale update returns a typed conflict and cannot overwrite the committed execution.

## DAG rules

- Node IDs are unique and bounded.
- Dependencies must exist.
- Self-dependencies and cycles are rejected.
- A task becomes ready after all dependencies satisfy their completion policy.
- Independent ready nodes may be assigned in parallel.
- Approval nodes become `waiting_approval` rather than executable tasks.
- A failed node retries while attempts remain.
- After retries are exhausted, an explicit fallback may be activated.
- Side effects remain the responsibility of task handlers and must be idempotent or compensatable.

## Lock rules

A resource is identified by structured workspace and path segments. Absolute paths and traversal segments are rejected.

Compatibility:

| Existing | Requested | Overlap | Result |
|---|---|---|---|
| Shared | Shared | Yes | Allowed |
| Shared | Exclusive | Yes | Conflict |
| Exclusive | Shared | Yes | Conflict |
| Exclusive | Exclusive | Yes | Conflict |
| Any | Any | No | Allowed |

The fencing token increases on every successful acquisition. Heartbeat and release require the current owner and token. Protected writes validate token/resource coverage and expiry immediately before commit.

## Persistence

Tables:

- `workflow_executions` — versioned JSON execution aggregate;
- `lock_leases` — current leases and serialized structured resource;
- `lock_sequence` — monotonic fencing-token source;
- shared `outbox` — workflow and lock audit events.

Workflow state plus events commit together. Lock acquisition/heartbeat/release plus events commit together.

Local SQLite mode uses one Runtime process and serializes acquisition operations. The future PostgreSQL adapter must use database transaction locking and preserve the same repository contract.

## Mandatory tests

- cycle rejection;
- fan-out readiness;
- stale workflow version conflict;
- approval persistence;
- workflow event atomicity;
- parent/child lock conflict;
- `src` does not conflict with `src2`;
- shared/shared overlap is allowed;
- stale fence is rejected;
- expired lease cannot authorize a write;
- lock events are written to outbox.
