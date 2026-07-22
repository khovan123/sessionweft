# RFC-0005: Durable Scheduler Claims

- Status: Implemented baseline
- Date: 2026-07-22

## Purpose

Define the first durable multi-agent scheduler contract: register per-node requirements, atomically claim ready Workflow nodes for running Agents, and complete or fail claims without duplicating state transitions.

## Data model

### Scheduler Plan

A plan belongs to one Workflow and Session. It maps task node IDs to optional Agent role and capability requirements. Missing entries mean no additional requirement beyond Agent availability.

### Task Claim

A claim records:

- Session, Workflow, node and attempt;
- Agent identity;
- Runtime task ID;
- deterministic idempotency key `workflow_id:node_id:attempt`;
- claim status;
- Workflow and Agent versions after the latest claim transition;
- timestamps.

## Claim algorithm

A claim transaction must:

1. load Plan, Workflow and Agent;
2. reject cross-Session ownership;
3. require a running Agent without an existing task;
4. scan ready nodes in Workflow definition order;
5. match role/capabilities;
6. reject an existing active claim for the same node;
7. call Workflow `start_node` and Agent `assign_task`;
8. persist both aggregates, the claim and all outbox events atomically.

No matching node returns `None` without state mutation.

## Completion and failure

Completion and failure load the active claim, validate Agent task ownership, transition the Workflow node, release the Agent task, update the claim and append events in one transaction.

Repeating completion of an already completed claim, or failure of an already failed claim, returns current state without a second mutation. Conflicting terminal transitions are rejected.

## Local and service modes

SQLite local mode serializes claim acquisition inside one Runtime process and uses a partial unique index for active claims. PostgreSQL service mode must use database transaction/locking semantics and the same externally observable contract.

## Deferred work

- stale-Agent claim release and task handover;
- scheduler polling loop and backoff;
- lock and approval prerequisites;
- provider/tool execution integration;
- cancellation and shutdown draining;
- metrics and operational controls.
