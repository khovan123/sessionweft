# ADR-0008: Runtime-Controlled Agent, Tool, Process and Git Boundaries

- Status: Accepted
- Date: 2026-07-22
- Issues: #6, #7, #13, #15

## Context

SessionWeft coordinates coding agents that call tools, launch processes, access MCP servers and modify Git workspaces. These capabilities cannot be delegated directly to a model, plugin or client because durable state, policy and audit belong to Runtime.

## Decision

1. Persist agent lifecycle, heartbeat, task ownership and failures as versioned Runtime records.
2. Agents declare capabilities in a manifest and cannot use undeclared permissions.
3. Tool policy is default-deny. Every tool declares a self-named tool permission plus resource permissions.
4. High/critical-risk tools and configured permissions require an explicit scoped approval grant.
5. MCP is wrapped behind Runtime discovery, descriptor validation, policy and approval. Transport discovery never authorizes execution.
6. Processes are launched without a shell through an absolute executable allowlist.
7. Process working directories are canonicalized inside the workspace root.
8. Process environments are cleared and only allowlisted keys are supplied.
9. Every process has a timeout and bounded combined output.
10. The first Git adapter wraps the installed Git CLI as a correctness oracle.
11. Read-only Git operations use the restricted runner; mutations require fencing validation immediately before execution.
12. Agent/API/IDE clients remain stateless with respect to durable execution state.

## Consequences

- The first process runner does not provide kernel-level CPU/memory sandboxing. Production plugin isolation still requires a separate process/container/WASM layer.
- PTY streaming and interactive input remain later extensions.
- MCP official SDK integration is an adapter task; policy contracts are already independent of transport.
- Git CLI availability is a runtime dependency for Git features but not for Session core.
- Approval grants require persistent one-time consumption before high-risk production use.

## Alternatives

- Permit models to call MCP tools directly: rejected.
- Run commands through `sh -c`: rejected due to injection and policy ambiguity.
- Share the full parent environment: rejected due to secret exposure.
- Treat heartbeat only as in-memory state: rejected because handover would not survive restart.
- Use gitoxide before a Git conformance corpus exists: deferred.
