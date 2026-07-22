# Fenced Git Worktree Allocation

Status: first implementation slice for issue #31.

## Purpose

Each scheduler Claim that changes a repository receives one isolated Git branch and worktree. Runtime state is persisted before the external Git command so interrupted allocation can be reconciled instead of becoming an unaudited worktree.

## Lifecycle

`provisioning → ready → abandoned → cleaned`

A provisioning failure transitions to `failed`. Failed or abandoned records may be cleaned. A stale provisioning record is reconciled by inspecting the expected worktree HEAD: an existing valid worktree becomes ready; a missing or unreadable worktree becomes failed.

## Durable identity

Every record stores:

- Session, Claim and Agent IDs;
- workspace ID and repository root;
- branch name and worktree path;
- base and head commits;
- lock ID, fencing token and lease expiry;
- lifecycle status, timestamps and sanitized failure information.

The Claim ID, branch name and worktree path are unique. Repeating the same reservation for the same Claim returns the existing record.

## Fence gate

Reservation is accepted only when the persisted lock lease:

- exists and is unexpired;
- belongs to the same Session;
- is owned by the Agent ID;
- covers the same workspace;
- has the exact fencing token supplied by the scheduler.

The lease expiry from storage replaces the caller-supplied expiry in the durable record.

## Git CLI isolation

The local adapter invokes `git` directly through process arguments and never through a shell. Allocation verifies the base commit, creates a dedicated branch and worktree, and reads the resulting HEAD. Cleanup removes the registered worktree and branch idempotently.

## Next slice

Merge queue state, reviewer/test gates, rebase and merge fence checks, conflict task creation and rollback are implemented after this allocation foundation.
