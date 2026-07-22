# Architecture Sign-off — `0.1.0-rc.1`

Decision: **Approved for Release Candidate; not approved for General Availability**.

Reviewer: `sessionweft-automation`  
Review type: automated architecture conformance review  
Date: 2026-07-22

## Evidence reviewed

- Runtime-owned durable state and Control Plane boundaries.
- Session/Workflow/Agent optimistic concurrency.
- transactional Outbox and idempotent Inbox.
- scheduler claims, stale recovery and handover.
- hierarchical locks and fencing.
- crash-safe Provider/Tool execution ledger.
- isolated Git worktrees and CAS merge queue.
- MCP SDK adapter, approval consumption and process sandbox.
- revision-aware workspace graph.
- PostgreSQL/JetStream service mode.
- resumable client protocol, Runtime-owned PTY, TUI and VS Code clients.

## Findings

- No architecture violation blocks an RC.
- SQLite local mode remains independent of PostgreSQL/NATS.
- Providers, plugins, IDE and CLI do not own Session state.
- GA remains blocked until a human architecture owner reviews capacity evidence, upgrade compatibility and deployment topology for the target environment.
