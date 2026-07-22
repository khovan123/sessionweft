# Upgrade and Rollback

## Compatibility rule

SessionWeft schema and event changes are additive by default. A release may write a new field only after the previous supported binary can safely ignore it. Destructive column removal, event reinterpretation and fencing-token reset are prohibited in the same release that introduces a replacement.

## Pre-upgrade gate

1. Verify the release archive checksum, SBOM and provenance attestation.
2. Run `sessionweft-release-gate --level rc` against the included policy/evidence.
3. Take verified PostgreSQL and JetStream backups.
4. Confirm no Critical/High findings and no unresolved migration warning.
5. Record current Runtime/worker instance IDs, binary versions and event schema versions.
6. Verify Outbox backlog and Inbox failures are below alert thresholds.

## Rolling upgrade

1. Deploy the new binary to one non-leader Runtime instance.
2. Wait for readiness and confirm it can read existing Sessions, claims, locks and cursors.
3. Observe one complete Outbox polling and JetStream ack/retry window.
4. Stop one old scheduler/execution/merge worker and start its new replacement.
5. Confirm no duplicate task owner or exclusive lock owner.
6. Repeat for remaining workers and Runtime instances one at a time.
7. Keep the previous release artifacts and database backup until the compatibility window closes.

Clients are stateless adapters. CLI, TUI and VS Code can be upgraded independently as long as the Runtime advertises a supported protocol version.

## Rollback

Rollback is permitted when the new binary can be replaced without restoring data. Redeploy the previous compatible binary one instance at a time and verify readiness, event cursor resume and worker ownership.

Restore data only when:

- a migration corrupts or removes data;
- the previous binary cannot read the new schema safely;
- event semantics cannot be repaired through a compatible consumer.

When a restore is required:

1. stop all writers;
2. restore PostgreSQL and JetStream into isolation;
3. validate Session/Outbox/Inbox/claim/lock invariants;
4. point one previous-version Runtime at the restored environment;
5. verify before traffic cutover.

## Forbidden rollback actions

- manually decrementing fencing tokens;
- deleting Inbox rows to force replay;
- marking Outbox rows published without matching stream evidence;
- deleting active task claims or lock leases while owners may still run;
- force-updating Git target refs;
- retrying an `Uncertain` external execution automatically.

## Compatibility drill

Run:

```bash
POSTGRES_CONTAINER=sessionweft-hardening-postgres \
  bash scripts/drills/migration-compatibility.sh
```

The drill creates a legacy sentinel, runs migrations twice with different Runtime instance IDs and proves legacy data remains readable.
