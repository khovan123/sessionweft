# Incident Response Runbook

## Severity

- **SEV-1:** data loss, secret exposure, stale-fence mutation, duplicate external side effect or complete service outage.
- **SEV-2:** SLO breach, delayed event processing, unavailable Provider/Tool path or failed restore drill without confirmed data loss.
- **SEV-3:** degraded performance, isolated client failure or non-critical operational defect.

## First 15 minutes

1. Assign incident commander and operations lead.
2. Record UTC start time, affected commit, Runtime instance IDs and deployment region.
3. Preserve PostgreSQL, JetStream and structured-log evidence.
4. Stop destructive or external side effects when consistency is uncertain.
5. Do not manually alter fencing tokens, claims, Inbox rows, Outbox attempts or Git refs.
6. Decide whether to isolate one Runtime, stop all writers, disable plugins or begin rollback.

## Consistency checks

Run before resuming mutations:

```sql
SELECT task_id, owner_id, claim_token, expires_at
FROM sessionweft_task_claims
WHERE expires_at > NOW();

SELECT workspace_id, owner_id, fencing_token, expires_at
FROM sessionweft_locks
WHERE expires_at > NOW()
ORDER BY workspace_id, fencing_token;

SELECT COUNT(*) AS pending_outbox
FROM sessionweft_outbox
WHERE published_at IS NULL;

SELECT consumer_name, COUNT(*) AS incomplete
FROM sessionweft_inbox
WHERE consumed_at IS NULL
GROUP BY consumer_name;
```

Compare active task and lock owners with live Runtime instance IDs. An expired owner may be recovered through normal lease expiry; do not transfer ownership by editing rows.

## Common incidents

### Runtime crash loop

- Stop the failing instance.
- Confirm migrations are compatible with the previous release.
- Start one known-good instance in isolation.
- Verify readiness, pending Outbox age and active claims before restoring replicas.

### PostgreSQL unavailable

- Stop Runtime writers after bounded retries begin.
- Fail over or restore according to `backup-restore.md`.
- Confirm RPO metadata and row counts before reopening traffic.

### JetStream unavailable or partitioned

- Keep PostgreSQL available; committed Outbox rows remain authoritative.
- Restore NATS connectivity and verify stream/consumer configuration.
- Observe Outbox draining and Inbox idempotency before closing the incident.

### Provider outage

- Disable or reroute the affected Provider adapter.
- Preserve committed user input and execution-ledger state.
- Do not retry `Running` or `Uncertain` external actions without reconciliation.

### Suspected secret exposure

- Revoke and rotate the credential immediately.
- Disable affected Provider/plugin/deployment path.
- Search logs, Session/Memory records, artifacts and Git history.
- Treat repository history rewrite as a separate reviewed operation.

### Stale fence or duplicate side effect

This is SEV-1. Stop all related workers, preserve the claim/lease/execution ledger and block the release. Resume only after the invariant violation has a regression test and security sign-off.

## Resolution and follow-up

An incident closes only after:

- service health and consistency checks are green;
- the error budget impact is calculated;
- temporary credentials or bypasses are removed;
- the fix includes a deterministic regression test;
- runbooks, alerts and SLO assumptions are updated;
- a post-incident review records timeline, root cause and actions.
