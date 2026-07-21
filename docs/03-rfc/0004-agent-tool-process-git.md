# RFC-0004: Agent, Tool, Process and Git Execution Boundary

- Status: Accepted for implementation
- Date: 2026-07-22
- ADR: ADR-0008

## Scope

This RFC adds the first controlled execution boundary:

- versioned agent manifests and lifecycle;
- SQLite agent persistence and audit outbox;
- heartbeat and stale-agent discovery;
- default-deny tool policy and scoped approvals;
- tool registry and MCP transport wrapper;
- no-shell restricted process execution;
- Git CLI read operations and fenced mutations.

Deferred:

- official MCP Rust SDK transport adapter;
- persistent one-time approval consumption;
- process CPU/memory/container limits;
- PTY streaming and interactive terminal input;
- plugin process supervisor and WASM isolation;
- gitoxide adapter and full merge queue;
- API/CLI/IDE endpoints for these contracts.

## Agent lifecycle

Agent records contain:

- Runtime-generated agent ID;
- Session ID;
- optimistic version;
- role and capability manifest;
- status;
- heartbeat timestamp and timeout;
- current task ownership;
- sanitized last error;
- timestamps.

Allowed lifecycle:

```text
registered -> running -> stopped -> running
                    \-> failed
```

Task ownership is explicit. A running agent may own at most one task in this baseline. Stale running agents are queryable so the scheduler can expire leases and hand work to another agent.

Agent mutation and audit events commit atomically.

## Tool policy

Every descriptor contains:

- name and version;
- JSON input schema;
- risk level;
- required permissions;
- a `Tool(name)` permission matching the descriptor name.

Evaluation order:

1. verify agent/session scope;
2. verify agent declares every permission;
3. reject explicit deny;
4. require approval for configured permissions;
5. deny permissions absent from the allowlist;
6. require approval for high/critical risk;
7. invoke only after an allow or valid scoped approval.

Approval scope includes Session, agent, tool and expiration. Persistent one-time consumption is required before exposing critical tools in service mode.

## MCP boundary

`McpTransport` exposes tool discovery and invocation. `McpGateway` validates every remote descriptor and runs the same Runtime policy before calling the transport. Tests must prove a denied tool does not reach the MCP transport.

The official MCP Rust SDK implements the transport later; it must not own Session state or authorization.

## Process boundary

`RestrictedProcessRunner` requires:

- canonical workspace root;
- alias-to-absolute executable allowlist;
- exact argument vector without shell parsing;
- canonical working directory under root;
- empty inherited environment plus allowlisted keys;
- bounded timeout;
- bounded combined stdout/stderr;
- kill-on-drop behavior.

Output limit violations and timeout are terminal typed errors. Request arguments and environment values are not logged by default.

## Git boundary

The baseline Git adapter uses the restricted runner and supports:

- porcelain status;
- diff without external diff drivers;
- staged commit;
- branch/worktree creation.

Read methods do not mutate. Every mutation receives a structured `GitFence`; a Runtime `FenceValidator` validates owner, resource coverage, token and lease expiry immediately before the command.

Git errors retain exit code and sanitized stderr. Recovery/rollback for merge and commit sequences remains a later RFC extension.

## Mandatory tests

- agent lifecycle and version conflict;
- task ownership prevents unsafe stop;
- agent state and outbox commit atomically;
- stale agent is discoverable;
- default-deny policy blocks invocation;
- MCP transport is not called after denial;
- high-risk tool requires scoped approval;
- unlisted process program is rejected;
- workspace escape and unapproved environment are rejected;
- timeout/output limits return typed errors;
- Git status matches a temporary repository;
- Git mutation requires a valid fence.
