# Durable Git Merge Queue

Status: second implementation slice for issue #31.

## Purpose

The Runtime owns a single durable merge queue. A worktree may enter the queue only after allocation is ready and its current workspace fence remains valid. Queue ordering, review decisions, test results, cancellation and ownership transitions are persisted and audited.

## Ordering

Each entry receives a monotonically increasing sequence. Eligible work is ordered by:

1. higher priority first;
2. lower sequence first for equal priority.

Only one entry may be `claimed` or `merging` at a time. Claiming uses both a process guard and a partial unique database index so concurrent workers cannot own different merge entries.

## Gates

An entry remains in `queued` until both gates pass:

- reviewer approval with reviewer ID and optional note;
- successful named test suite with optional summary.

Failed or missing gates make an entry ineligible. Gate decisions may only change while the entry is queued.

## Fence validation

The Runtime revalidates the persisted lease:

- when the worktree is enqueued;
- immediately before an item is claimed;
- before merge starts;
- before a rebased head is accepted;
- before the final merge result is persisted.

The lease must still belong to the same Session and Agent, cover the workspace, carry the exact fencing token and remain unexpired. The durable worktree must remain ready and its head must match the queue snapshot.

An eligible entry with a stale fence is transitioned to `failed` and audited instead of being returned to a merge worker.

## Lifecycle

`queued → claimed → merging → merged`

Alternative terminal paths are `cancelled`, `conflict` and `failed`. Cancellation is allowed before merge begins. Conflict paths are persisted and become conflict-resolution tasks in the next slice.

## Next slice

The next implementation adds the local rebase/merge executor, persisted conflict tasks, interrupted-merge recovery and rollback.
