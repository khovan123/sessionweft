# Scheduler Task Handover and Retry

Status: implementation and verification slice for issue #30.

## Input

Handover starts from a persisted `released` claim. The previous claim ID is the durable history link and is never rewritten.

## Replacement selection

The scheduler selects the first deterministic candidate ordered by `updated_at` and Agent ID that:

- belongs to the same Session;
- is running;
- has no current task;
- is not stale at the supplied scheduler timestamp;
- is not the previous Agent;
- matches the SchedulerPlan role and capability requirement.

## Atomic transition

One transaction must:

1. revalidate the previous released claim;
2. return an existing active claim for the same node when a handover was already committed;
3. verify the Workflow node is `ready`;
4. start the Workflow node for the replacement Agent;
5. assign the deterministic retry task ID to the Agent;
6. insert a new active claim with the incremented attempt and idempotency key;
7. persist Workflow and Agent versions;
8. append Workflow, Agent and `scheduler.claim_handed_over` events to the outbox.

## No replacement

When no matching Agent is available, no state is changed and the caller receives `None`. A polling loop may retry with backoff later.

## Idempotency

Repeating handover for the same released claim returns the current active claim for the Workflow node and does not create another task or side effect key.

## Verification

The branch must pass read-only dependency lockfile, rustfmt, Clippy and all workspace tests after the one-shot formatter is removed.
