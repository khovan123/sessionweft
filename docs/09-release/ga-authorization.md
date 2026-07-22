# General Availability Authorization — SessionWeft 0.1.0

Date: 2026-07-22  
Authorizing authority: `khovan123`  
Verified repository permission: `admin`  
Analysis executor: `sessionweft-automation`

## Authorization

The repository owner explicitly authorizes automated research and evidence analysis for the General Availability reviews covering Architecture, Security and Operations. The owner also authorizes the resulting decisions to be applied to the SessionWeft 0.1.0 GA evidence when every release-blocking technical gate passes.

The approving authority is the human repository owner, `khovan123`. Automation performs the delegated analysis and prepares the evidence; it is not represented as a human reviewer.

## Approved GA scope

- SQLite local single-user Runtime mode.
- Authenticated single-tenant team service mode using PostgreSQL and NATS JetStream.
- CLI, Ratatui TUI and VS Code clients acting only as Runtime adapters.
- Provider and Tool execution through the durable execution ledger.
- Official MCP SDK integration with one-time approvals.
- Linux production plugin isolation using bubblewrap.
- Git worktree isolation, fencing and compare-and-swap merge queue.

## Explicit exclusions

- Multi-tenant SaaS isolation and billing.
- Production plugin sandbox guarantees on non-Linux operating systems.
- Any deployment that disables authentication, durable storage, fencing, approval or audit controls required by the release policy.
- Any future provider, plugin or deployment adapter that has not passed the same release gates.

## Non-waiver conditions

This authorization cannot override or waive:

- a failed required release gate;
- an open Critical or High security finding;
- failed backup/restore, migration or chaos evidence;
- missing artifact provenance, checksums or SBOM;
- a stale fencing token, duplicate side effect or workspace/plugin escape;
- a mismatch between evidence and the exact tested commit.

## Decision model

GA sign-off records use:

- `reviewer: khovan123` because the owner is the approving authority;
- `human: true` because the authority is a human repository owner;
- `decision: approved_for_ga` only after delegated analysis and CI verification;
- evidence links to this authorization and the corresponding GA review report.

Related tracking issue: #64.
