# PostgreSQL and JetStream Service Mode

Status: production baseline for issue #34.

## Contract

Local SQLite mode remains the reference domain contract. Service mode replaces persistence and event transport only; it does not move Session, Workflow, Agent, Memory or Lock ownership into PostgreSQL or NATS.

## PostgreSQL repositories

`sessionweft-service-postgres` provides adapters for:

- Session and transactional Outbox;
- Workflow execution with optimistic version checks;
- Agent state and stale-heartbeat queries;
- Memory records and supersession/deletion transactions;
- hierarchical lock leases with workspace-level serialization and monotonically increasing fencing tokens;
- expiring task claims shared by multiple Runtime instances;
- idempotent consumer Inbox claims.

Every domain mutation and its emitted events commit in one PostgreSQL transaction. Updates use expected-version compare-and-set. Outbox claims use `FOR UPDATE SKIP LOCKED` plus an expiring claim lease, so multiple publishers do not publish the same pending row concurrently. A crashed publisher releases ownership through claim expiry.

## Lock concurrency

Acquisition first locks the workspace guard row. The transaction then reads every non-expired lease for that workspace with `FOR UPDATE`, applies the domain overlap rules, allocates the next fencing token and inserts the lease. This serializes competing acquisitions without weakening directory/file/symbol hierarchy semantics.

## JetStream transport

`sessionweft-jetstream` implements the existing `EventTransport` boundary. The configured stream owns `sessionweft.events.>` subjects and uses durable pull consumers with explicit acknowledgements.

Consumers apply:

1. JSON and event schema validation;
2. PostgreSQL Inbox claim with a processing TTL;
3. idempotent handler invocation;
4. ACK after Inbox completion;
5. delayed NAK for retryable failures;
6. dead-letter publication after the configured delivery limit.

Closing or restarting a Runtime does not delete durable consumers, Outbox rows, Inbox history, task claims or locks.

## Compatibility and replay

Each consumer declares the maximum event schema it supports. Events above that version are dead-lettered rather than interpreted. Durable consumers default to replay from all retained events. The Inbox primary key `(consumer_name, event_id)` makes replay idempotent.

## Integration evidence

The service-mode CI gate starts PostgreSQL and NATS JetStream and runs ignored integration tests explicitly. Two independent `PostgresServiceDatabase` instances race for one task claim and one conflicting lock; exactly one succeeds. A duplicated JetStream event is handled once through the PostgreSQL Inbox.

## Local mode

No SQLite repository or local broadcast transport is removed. Applications select service mode through dependency injection and environment configuration. Development and single-user installations continue to operate without PostgreSQL or NATS.
