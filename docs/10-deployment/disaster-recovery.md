# Disaster Recovery

## Objectives

For service mode `0.1.0-rc.1`:

- Recovery Time Objective: **30 minutes**.
- Recovery Point Objective: **5 minutes**.
- Local mode committed-state RPO: **0 seconds** after a successful transaction acknowledgement.

These are release targets, not claims about an untested deployment. Every production environment must execute and record the drills below.

## Backup set

A recoverable service-mode backup contains:

1. PostgreSQL custom-format dump;
2. JetStream stream snapshot;
3. Runtime commit and release policy version;
4. database schema inventory;
5. stream name, subject prefix, first/last sequence and durable consumers;
6. checksum manifest stored outside the primary failure domain.

PostgreSQL and JetStream snapshots should be taken in the same maintenance window. Outbox and Inbox identities make replay safe but do not replace coordinated backups.

## Automated PostgreSQL drill

Run:

```bash
POSTGRES_CONTAINER=sessionweft-hardening-postgres \
  bash scripts/drills/postgres-backup-restore.sh
```

The drill must:

- create a unique marker;
- generate and list a custom-format dump;
- restore into a separate database;
- verify the marker and Session count;
- remove the isolated database without modifying production data.

## JetStream drill

1. Create a stream snapshot with the NATS CLI.
2. Restore into an isolated NATS account or server.
3. Verify stream subjects, message count, durable names and ack floors.
4. Connect a disposable consumer using a separate Inbox consumer name.
5. Replay duplicate events and verify one handler invocation.
6. Destroy the isolated environment after evidence is collected.

## Runtime recovery sequence

1. Freeze writers and record the incident timestamp.
2. Preserve failed database and NATS volumes for forensics.
3. Provision isolated PostgreSQL and NATS instances.
4. Restore PostgreSQL; validate schema, Session count, active locks, task claims, pending Outbox and Inbox uniqueness.
5. Restore JetStream; validate stream and consumer metadata.
6. Start exactly one Runtime instance in readiness-check mode.
7. Confirm Session reads, event publishing and durable cursor resume.
8. Start scheduler/execution/merge workers one at a time.
9. Confirm expired claims recover and no active claim/lock is duplicated.
10. Add the second Runtime only after one full retry/ack window.

## Corruption handling

- Never run repair commands directly on the only copy.
- Clone the volume or restore into isolation first.
- A workspace graph snapshot with a revision mismatch is discarded and rebuilt from source files.
- A malformed event remains in Outbox/Inbox evidence and is routed to DLQ after policy limits.
- An execution left `Running` after process loss becomes `Uncertain`; operators reconcile it rather than retrying blindly.

## Recovery acceptance

The drill passes only when:

- the restore finishes within 30 minutes;
- the verified backup is no older than five minutes for the measured window;
- all sampled Sessions and events are present;
- no duplicate active task claim or exclusive lock exists;
- duplicate event replay invokes the handler once;
- the event cursor resumes without going backwards;
- all operator actions and checksums are recorded.
