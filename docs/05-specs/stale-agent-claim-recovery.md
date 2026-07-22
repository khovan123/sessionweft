# Stale-Agent Claim Recovery

Status: implementation slice for issue #30.

## Trigger

An active scheduler claim is recoverable when its owning Agent is still persisted as `running` but `AgentRecord::is_stale_at(now)` returns true.

## Atomic transition

For each stale active claim, one SQLite transaction must:

1. reload and revalidate the active claim;
2. verify Agent task ownership matches the claim task ID;
3. fail the running Workflow node through the existing Workflow state machine;
4. mark the stale Agent as failed and clear task ownership;
5. mark the claim as released;
6. persist Workflow, Agent and Claim versions;
7. append correlated recovery and handover events to the transactional outbox.

## Retry behavior

The Workflow state machine remains authoritative. If the failed node has attempts remaining, it returns to `ready` and `scheduler.handover_required` is emitted. If attempts are exhausted, normal Workflow failure/fallback rules apply.

## Idempotency

Recovery only selects active claims. Re-running recovery after a successful transaction produces no duplicate transition or outbox event.
