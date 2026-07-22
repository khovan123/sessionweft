# Service Mode Operations

## Start

```bash
docker compose -f deploy/docker-compose.service.yml up -d postgres nats
```

Add `--profile vectors qdrant` only when vector storage is enabled.

## Configuration

```text
SESSIONWEFT_DATABASE_URL=postgres://sessionweft:<secret>@postgres:5432/sessionweft
SESSIONWEFT_NATS_URL=nats://nats:4222
SESSIONWEFT_RUNTIME_INSTANCE_ID=<stable-instance-id>
```

Every Runtime instance must have a unique stable instance ID. Database and NATS credentials must come from the deployment secret manager rather than committed environment files.

## PostgreSQL backup

```bash
pg_dump --format=custom --no-owner \
  --dbname="$SESSIONWEFT_DATABASE_URL" \
  --file="sessionweft-$(date -u +%Y%m%dT%H%M%SZ).dump"
```

Record the application commit, schema version and JetStream stream sequence beside the backup. Verify every backup with `pg_restore --list` and a restore into an isolated database.

## PostgreSQL restore

1. Stop all Runtime writers.
2. Create an empty restore database.
3. Run `pg_restore --clean --if-exists --no-owner` against the isolated database.
4. Start one Runtime in migration-check mode.
5. Verify Session counts, active locks, pending Outbox rows and Inbox uniqueness.
6. Switch the service database endpoint only after verification.

Never restore over a running production database.

## JetStream backup

Use the NATS CLI snapshot API for the configured stream:

```bash
nats stream backup SESSIONWEFT_EVENTS ./jetstream-backup
```

The PostgreSQL Inbox makes replay safe, but the PostgreSQL backup and JetStream snapshot should be taken in the same maintenance window when strict point-in-time recovery is required.

## JetStream restore

```bash
nats stream restore ./jetstream-backup
```

Restore into an isolated NATS account first. Verify stream subjects, consumer durable names, last sequence and dead-letter subjects before reconnecting Runtime instances.

## Migration rollback

Schema changes are additive by default. Before a destructive migration:

1. take and verify PostgreSQL and JetStream backups;
2. deploy code that can read both old and new schema forms;
3. apply the migration;
4. observe one full Outbox and consumer retry window;
5. remove old columns only in a later release.

Rollback means redeploying the previous compatible binary and restoring the pre-migration database only when the new schema cannot be read safely. Do not manually decrement fencing tokens, task claims, Outbox attempts or JetStream sequences.

## Failure recovery

- Expired Outbox claims become eligible for another publisher.
- Expired task claims may be acquired by another Runtime.
- Expired locks no longer validate their fencing token.
- Inbox processing claims expire and allow JetStream redelivery.
- Completed Inbox rows remain permanent idempotency evidence according to retention policy.
- Poison events move to the configured dead-letter subject after the maximum delivery count.
