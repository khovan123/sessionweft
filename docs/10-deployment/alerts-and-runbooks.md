# Alerts and Runbooks

This document is the response target referenced by `deploy/observability/prometheus-alerts.yml`.

## Runtime unavailable

Alert: `SessionWeftRuntimeUnavailable`.

1. Confirm `/health/live` and `/health/ready` from inside and outside the deployment network.
2. Check Runtime JSON logs for migration, bind, authentication configuration and repository errors.
3. Verify PostgreSQL with `pg_isready` and NATS monitoring at `/healthz` or `/jsz`.
4. Do not start a replacement with the same `SESSIONWEFT_RUNTIME_INSTANCE_ID` while the old process may still run.
5. Start one replacement Runtime and verify event cursor continuity before scaling out.
6. If startup follows a migration, use the rollback procedure rather than editing schema manually.

Escalate immediately when both Runtime instances are unavailable for five minutes or committed Session reads fail.

## Outbox backlog

Alert: `SessionWeftOutboxBacklogGrowing`.

1. Query the oldest unpublished row and `publish_attempts` in `sessionweft_outbox`.
2. Verify JetStream connectivity and subject/stream compatibility.
3. Check for a poison event or unsupported schema version.
4. Restart one publisher only; expired claims allow another Runtime to resume safely.
5. Never mark rows published manually without verifying the matching JetStream event.
6. Route permanently invalid envelopes to the documented dead-letter subject and record the event ID.

Exit condition: backlog is decreasing for at least two polling windows and the oldest event age is below the event-delivery SLO.

## Inbox retries

Alert: `SessionWeftInboxRetriesStuck`.

1. Inspect `consumer_name`, `event_id`, `event_type`, attempts and last error.
2. Determine whether the handler failure is transient, incompatible schema or invalid payload.
3. For transient failure, restore the dependency and allow NAK/redelivery.
4. For incompatible schema, deploy a compatible consumer before replay.
5. Do not delete completed Inbox rows; they are idempotency evidence.
6. Move poison events to DLQ only after the configured maximum delivery count.

## JetStream unavailable

Alert: `SessionWeftJetStreamUnavailable`.

1. Check NATS container/process and JetStream storage volume.
2. Verify stream name, subject prefix and durable consumer names.
3. Inspect disk capacity and NATS server logs.
4. Restore the stream from a verified snapshot only into an isolated account first.
5. Runtime Outbox rows remain authoritative for unpublished events; do not purge them during recovery.

## JetStream backlog

Alert: `SessionWeftJetStreamBacklogHigh`.

1. Compare stream message count with durable consumer pending/ack floors.
2. Identify slow or offline durable consumers.
3. Check Inbox failures and handler latency.
4. Scale consumers only when PostgreSQL Inbox uniqueness remains shared.
5. Verify backlog falls without duplicate handler invocation.

## Task claim saturation

Alert: `SessionWeftTaskClaimsSaturated`.

1. Compare active claims with running Agents and scheduler tick progress.
2. Find expired claims and stale Agent heartbeats.
3. Confirm stale recovery and handover workers are running.
4. Check lock/approval prerequisites that may block ready work.
5. Do not delete active claims; use expiry and fencing-aware recovery.

## Lock contention

No automatic release is allowed for a non-expired owner solely because contention is high.

1. Inspect resource hierarchy, owner, expiry and fencing token.
2. Verify the owning Agent heartbeat and task claim.
3. Cancel or fail the owner through Runtime policy if intervention is required.
4. Wait for release/expiry and ensure the next owner receives a strictly higher fencing token.

## Provider outage

1. Confirm the user input is committed before retry decisions.
2. Inspect execution ledger state: `Prepared`, `Running`, `Failed` or `Uncertain`.
3. Never automatically retry `Uncertain` side effects.
4. Switch provider only through Session provider selection and preserve Session identity.
5. Resume from persisted state after the provider recovers.

## Plugin incident

1. Cancel the invocation and terminate the sandbox process.
2. Revoke unconsumed approval grants and rotate any potentially exposed credentials.
3. Preserve plugin command, declared permissions, tool schema, correlation ID and sandbox logs.
4. Disable the plugin server registration until security review completes.
5. Treat filesystem/network escape or secret access as a Critical finding.

## Evidence collection

For every Critical/High incident preserve:

- UTC start/end time;
- Runtime commit and instance IDs;
- Session/workflow/task/event/lock IDs;
- relevant structured logs and metrics;
- database and JetStream snapshot identifiers;
- operator actions and approvals;
- root cause, corrective action and regression test.
