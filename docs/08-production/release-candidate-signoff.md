# SessionWeft Release-Candidate Sign-off

Release scope: first authenticated single-tenant Runtime release with local SQLite mode and PostgreSQL/JetStream service mode.

Architecture: APPROVED_FOR_RC

Security: APPROVED_FOR_RC

Operations: APPROVED_FOR_RC

GA: NOT_APPROVED

## Conditions of approval

The three RC approvals are valid only for a commit where all required GitHub checks pass:

- locked Rust dependency graph, format, Clippy and workspace tests;
- PostgreSQL/JetStream service-mode integration tests;
- production hardening chaos, backup/restore and capacity jobs;
- dependency advisory scan and committed-secret scan;
- SBOM generation and release-evidence verification;
- VS Code extension typecheck/build gate.

A failed, skipped or manually bypassed release-blocking job invalidates this approval for that commit.

## Architecture review

Approved controls:

- Runtime remains the sole owner of durable Session and execution state.
- PostgreSQL mutations commit state and Outbox atomically.
- JetStream uses at-least-once delivery with Inbox idempotency.
- task claims, locks and Git mutations use lease/fencing authority.
- clients are stateless adapters with cursor-based reconnect.
- Workspace intelligence is revision-bound and cannot escape the canonical root.

No architecture blocker with Critical or High impact is open for the RC scope.

## Security review

Approved controls:

- authenticated non-loopback API access;
- default-deny Tool and MCP authorization;
- one-time durable approval consumption;
- bubblewrap plugin isolation and cleared environment;
- no-shell process execution and normalized Git/workspace paths;
- secret scan, dependency audit, SBOM and provenance release gate.

No known Critical or High security finding is accepted for RC. A new finding at either severity automatically revokes this sign-off.

## Operations review

Approved controls:

- numeric SLO, RTO, RPO and capacity targets;
- health checks, alert rules, dashboard and incident runbooks;
- PostgreSQL backup/isolated restore drill;
- NATS partition/recovery and event-redelivery tests;
- migration idempotency and persisted-data corruption tests;
- rolling-upgrade and rollback procedure;
- checksummed release bundles, SBOM and build provenance.

## GA restriction

This document approves an internal/public Release Candidate only. General Availability requires a separate sign-off after sustained SLO evidence, an external security review for the intended deployment model, operator restore drills in the target environment and resolution of every RC incident.
