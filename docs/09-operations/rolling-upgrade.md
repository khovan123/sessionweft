# Rolling Upgrade and Rollback Runbook

## Compatibility policy

- Database migrations are additive for at least one release window.
- A new binary must read the previous schema and current schema during rollout.
- Event consumers reject unknown breaking schema versions and route poison events to the DLQ.
- Client protocol additions are backward compatible within the declared protocol version.
- Destructive schema changes require a separate release after old readers are removed.

## Pre-upgrade gate

1. Verify the target commit passed CI, service-mode, client, hardening and security workflows.
2. Verify checksums, SBOM and provenance for the exact artifact.
3. Complete PostgreSQL and JetStream backups.
4. Record active Runtime instances, task claims, locks, pending Outbox age and consumer lag.
5. Confirm the previous release artifact and configuration remain available for rollback.

## Rolling upgrade

1. Remove one Runtime instance from traffic.
2. Allow its in-flight request grace period to complete.
3. Stop the instance; leases and task claims remain governed by expiry and fencing.
4. Deploy the new binary with the same stable Runtime instance ID only after the old process is stopped.
5. Verify `/health/live`, `/health/ready`, authentication and client protocol version.
6. Observe error rate, p99 latency, Outbox age, Inbox attempts and provider/tool failures for one full retry window.
7. Repeat for remaining instances one at a time.
8. Upgrade scheduler, execution and merge workers with the same drain-and-observe process.

Never run two processes with the same Runtime instance ID concurrently.

## Rollback

Rollback is allowed when the previous binary can read the migrated schema.

1. Stop the newly deployed instance.
2. Redeploy the previous attested artifact.
3. Verify health and the same consistency signals used during upgrade.
4. Continue instance by instance.

If the previous binary cannot read the new schema:

- stop all writers;
- restore the pre-upgrade database into an isolated target;
- verify recovery according to `backup-restore.md`;
- switch endpoints only after review.

Do not manually decrement schema versions, fencing tokens, JetStream sequences, task attempts or client cursors.

## Upgrade acceptance criteria

- At least one Runtime remains available throughout the service-mode rollout.
- No task or conflicting lock receives two active owners.
- No committed Session mutation is lost.
- Outbox and Inbox queues return to their pre-upgrade steady state.
- Existing CLI, TUI and VS Code clients reconnect from their durable cursor.
- Rollback rehearsal completes within the 30-minute RTO.
