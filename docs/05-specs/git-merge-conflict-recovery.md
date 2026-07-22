# Git Merge Execution, Conflict Recovery and Rollback

Status: final implementation slice for issue #31.

## Worker-side commits

A worker may stage and commit changes only through the fenced mutation service.

1. Validate the durable worktree and live workspace lease immediately before stage.
2. Reject an index that already contains staged changes.
3. Stage only normalized repository-relative paths declared by the request.
4. Revalidate the same Session, Agent, workspace, lock ID and fencing token immediately before commit.
5. If the second validation fails, unstage the declared paths and do not create a commit.
6. Persist the new worktree HEAD and Outbox event after commit.
7. If persistence fails, reset the branch to the previous HEAD with `--mixed` so file contents are preserved.

## Merge execution

The merge worker claims one eligible queue entry and validates the fence before entering `merging`.

### Target unchanged

When the target branch still equals the durable base commit, the worker verifies the source is a fast-forward descendant and executes an atomic compare-and-swap:

`git update-ref refs/heads/<target> <source-head> <expected-target>`

The update succeeds only when the target still matches the expected commit. A concurrent target change requeues the entry and resets reviewer/test gates.

### Target changed

When another merge advanced the target, the worker rebases the source branch inside its isolated worktree. A successful rebase updates both the durable worktree HEAD and queue snapshot in one database transaction. The entry returns to `queued`, and both reviewer and test gates return to `pending` before the rebased commit may merge.

## Conflicts

A rebase conflict is aborted in the source worktree. Conflicted paths are normalized and persisted in one `ConflictResolutionTask` per queue entry. The queue transitions to `conflict`, and the task records Session, Claim, worktree, source/target branches and source/target commits. Repeated reporting returns the existing task rather than creating duplicates.

## Interrupted execution

The worker periodically scans stale `merging` entries and inspects Git reality:

- target already equals the queued head: persist the merge as completed;
- source HEAD changed and contains the target: persist the completed rebase, update the worktree snapshot and requeue for renewed gates;
- rebase metadata exists: abort the interrupted rebase and fail the entry with audit history;
- worktree is missing or refs diverged unexpectedly: fail the entry without overwriting refs.

## Rollback

If the target compare-and-swap succeeds but persisting `merged` fails, the worker attempts a second compare-and-swap that restores the previous target commit. Rollback never overwrites a target that moved again. A commit persistence failure similarly resets the source branch to the previous HEAD while retaining working-tree changes.

## Process isolation

The merge worker is a separate supervised binary. It uses bounded reconciliation, one-active-entry queue storage, exponential backoff, structured logs and graceful cancellation. No Git operation is invoked through a shell.
