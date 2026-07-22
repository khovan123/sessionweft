# Project Status

Last updated: 2026-07-22  
Current phase: **General Availability — SessionWeft 0.1.0**  
Decision: **Approved for GA within the declared scope**

## Completed phases

- [x] Phase -1 — Capability Matrix
- [x] Phase 0 — Landscape Research
- [x] Architecture Review
- [x] ADR baseline
- [x] RFC and Production Specification
- [x] Phase 2 implementation
- [x] Production testing and chaos/recovery qualification
- [x] Release Candidate gate
- [x] Owner-authorized Architecture, Security and Operations GA review
- [x] General Availability gate

## GA scope

- SQLite local single-user Runtime mode.
- Authenticated single-tenant service mode using PostgreSQL and NATS JetStream.
- Runtime-owned Session, Workflow, Agent, Memory, Lock, Git, Provider, Tool and event state.
- CLI, Ratatui TUI and VS Code clients as stateless Runtime adapters.
- Official MCP SDK integration and one-time approvals.
- Linux production plugin sandbox using bubblewrap.
- Revision-aware workspace intelligence for Rust, TypeScript/JavaScript and Python.

## Explicit exclusions

- Multi-tenant SaaS isolation and billing.
- Production plugin sandbox guarantees outside Linux.
- Unqualified future provider, plugin, storage or deployment adapters.
- Deployments that disable required authentication, approval, fencing, durable storage or audit controls.

## Release guarantees

- Locked Rust dependency graph, rustfmt, Clippy with warnings denied and workspace tests.
- PostgreSQL/JetStream service-mode ownership, redelivery and recovery tests.
- Durable scheduler claims, stale-Agent handover and external side-effect idempotency.
- Hierarchical lock leases and fencing-token enforcement.
- Isolated Git worktrees, compare-and-swap merge queue and conflict recovery.
- Default-deny Tool/MCP execution with durable approval consumption.
- Backup/restore, migration, provider outage and network-partition drills.
- Secret scanning, dependency audit, SBOM, checksums and provenance attestations.
- Metrics, alerts, dashboard and operations runbooks.

## GA approval model

The human repository owner `khovan123` is the approving authority. `sessionweft-automation` performs delegated research and evidence analysis. The authorization, review reports, scope and residual risks are recorded under `docs/09-release/ga-*.md`.

The GA gate cannot pass when:

- required evidence identifies `TBD` instead of the tested commit;
- a required gate is failed or waived;
- a Critical or High security finding remains open;
- Architecture, Security or Operations GA approval is missing;
- the approving authority is not recorded as human;
- release evidence has no supporting artefacts.

## Release artefacts

- GA policy: `release/ga-policy-0.1.0.json`
- GA evidence template: `release/evidence/ga-0.1.0.json`
- GA workflow: `.github/workflows/ga-approval.yml`
- Release workflow: `.github/workflows/release.yml`
- GA record: `docs/09-release/general-availability.md`
