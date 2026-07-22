# MCP Plugin Isolation and Approval Audit

Status: implementation specification for issue #32.

## Runtime authority

MCP SDK clients and plugin processes never own Session, Agent, Task, approval, policy, memory or audit state. They receive only a bounded invocation after Runtime authorization.

## One-time approval consumption

High-risk or approval-required MCP invocations use a persisted `ApprovalGrant`.

The SQLite authority:

1. loads the grant inside a transaction;
2. verifies Session, Agent, tool and expiry;
3. rejects previously consumed grants;
4. updates `consumed_at` with `WHERE consumed_at IS NULL`;
5. records the invocation correlation ID;
6. writes `mcp.approval_consumed` to the Outbox in the same transaction.

The approval is consumed before the external plugin side effect. A transport failure does not make the approval reusable.

## stdio sandbox

Production stdio plugins run through an explicit bubblewrap launcher profile.

Default profile:

- `--die-with-parent`;
- `--new-session`;
- `--unshare-all`;
- no `--share-net` unless Network permission was explicitly granted;
- isolated `/proc`, `/dev` and `/tmp`;
- explicit read-only runtime roots;
- workspace bind is read-only or read-write according to policy;
- canonical workspace and working-directory validation;
- cleared environment plus explicit `--setenv` entries;
- direct plugin program and arguments after `--`.

Unsandboxed stdio remains available only through the lower-level official SDK adapter and must not be selected by the production plugin registry.

## Streamable HTTP

Remote MCP servers are constrained by endpoint scheme and host allowlists. HTTPS is required except explicitly allowed loopback development endpoints. URL-embedded credentials are rejected.

## Malicious plugin verification

The repository includes a real stdio MCP fixture with modes for:

- protocol-compatible normal discovery and invocation;
- duplicate tool names;
- non-object schema spoofing;
- initialization hang;
- oversized tool result;
- secret-environment probe.

Integration tests verify namespace normalization, timeout, output bounds, cleared environment and sandbox arguments.

## Cancellation and teardown

Every SDK operation owns a service lifecycle. Runtime cancellation or timeout cancels the MCP service token; stdio children use `kill_on_drop`. A plugin cannot survive Runtime cancellation as an owned Session component.

## Audit events

- `mcp.approval_issued`
- `mcp.approval_consumed`

Both events include Session, Agent, tool, grant and invocation correlation identifiers without embedding secrets or tool payloads.
