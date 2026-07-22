# Backup, Restore and Corruption Runbook

## Backup policy

Service mode uses PostgreSQL as durable authority and JetStream as event transport. Take both snapshots in the same maintenance window when strict point-in-time recovery is required.

- PostgreSQL full backup: at least every 24 hours.
- PostgreSQL WAL/PITR archive target: RPO no greater than five minutes.
- JetStream stream snapshot: after schema/configuration changes and before destructive maintenance.
- Retain one daily backup for 14 days and one weekly backup for 8 weeks.
- Encrypt backups and store them outside the primary failure domain.

Every backup record must include:

- UTC timestamp;
- application commit and release version;
- PostgreSQL server version and schema inventory;
- JetStream stream name and last sequence;
- checksum and storage location;
- verification result.

## Automated isolated restore drill

Run:

```bash
SESSIONWEFT_POSTGRES_CONTAINER=sessionweft-hardening-postgres \
  scripts/hardening/backup_restore_drill.sh
```

The script creates a custom-format dump, validates its catalog, restores into a new database and compares row counts for Sessions, Workflows, Agents, Memory, Locks, Outbox, Inbox and task claims. It never restores over the source database.

## Production restore

1. Stop all Runtime writers and record active instance IDs.
2. Preserve the failed database and current JetStream state for investigation.
3. Create an isolated PostgreSQL database or replacement cluster.
4. Restore the selected backup and replay WAL only to the approved recovery point.
5. Start one Runtime in migration/check mode.
6. Verify schema, Session counts, active claims/locks, pending Outbox, Inbox uniqueness and client event cursors.
7. Restore JetStream into an isolated account when a stream snapshot is required.
8. Connect one consumer and verify idempotent replay.
9. Switch endpoints only after architecture and operations approval.
10. Re-enable Runtime replicas gradually while observing alerts.

## Corruption handling

A JSON deserialization failure, impossible numeric value, invalid schema version or inconsistent fence is treated as corruption evidence.

- Do not delete or rewrite the row in place.
- Stop the affected mutation path.
- Export the row, transaction ID and correlated events.
- Determine whether a compatible reader or migration can recover it.
- Restore from backup when correctness cannot be proven.
- Add the malformed shape to a regression test before reopening traffic.

The hardening suite inserts a structurally invalid Session JSON object and verifies the repository returns a typed serialization error rather than panicking or silently accepting the row.

## Restore acceptance criteria

- No required table is missing.
- Row-count comparison is exact for the automated drill.
- No active task/lock has an impossible owner or expired fence accepted as current.
- Pending Outbox records can publish after recovery.
- Duplicate JetStream delivery does not repeat the handler side effect.
- The measured restore duration is within the 30-minute service-mode RTO.
