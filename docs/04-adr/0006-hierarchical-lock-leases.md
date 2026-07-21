# ADR-0006: Hierarchical Lock Leases and Fencing Tokens

- Status: Accepted
- Date: 2026-07-22
- Issue: #14

## Context

Parallel workers can modify overlapping workspace resources. A dead, paused or partitioned worker must not retain authority indefinitely or commit after a newer owner has acquired the resource.

## Decision

1. Lock hierarchy is Workspace → Directory → File → Symbol.
2. Parent and child resources overlap by path-segment prefix, not string prefix.
3. Shared locks may coexist only when every overlapping lock is shared.
4. Any overlapping exclusive lock conflicts.
5. Locks use bounded leases with heartbeat and expiration.
6. Every acquisition receives a monotonically increasing fencing token.
7. Protected writes must validate owner, resource coverage, token and expiry immediately before commit.
8. Acquire, heartbeat and release audit events commit atomically with lock state.
9. Local SQLite mode serializes acquisition through the single Runtime process.
10. Service mode must use PostgreSQL transaction locking; a process-local mutex is not a distributed lock.

## Consequences

- A stale worker cannot rely on lock ID alone; its fencing token becomes invalid after lease expiry/reacquisition.
- Clock behavior affects expiry. Service deployments require synchronized clocks and conservative TTLs.
- Fair wait queues and automatic ownership transfer remain later protocol extensions.
- Merge eligibility must validate fences before applying protected changes.

## Alternatives

- Permanent ownership flags: rejected because crash recovery cannot release them safely.
- Optimistic conflict detection only: insufficient for expensive concurrent agent edits.
- Path string prefix matching: rejected because `src` must not overlap `src2`.
- NATS-only locks: rejected because lock authority must be transactionally auditable with Runtime state.
