# Project Status

Last updated: 2026-07-23  
Current phase: **General Availability — SessionWeft 0.2.0**  
Decision: **Approved for GA after exact-commit qualification and immutable publication**

## Completed phases

- [x] Phase -1 — Capability Matrix
- [x] Phase 0 — Landscape Research
- [x] Architecture Review
- [x] ADR baseline
- [x] RFC and Production Specification
- [x] Phase 2 implementation
- [x] Production testing and chaos/recovery qualification
- [x] SessionWeft 0.1.0 General Availability
- [x] Phase 3 multi-tenant SaaS and billing
- [x] Portable production plugin sandbox
- [x] Adapter certification and activation enforcement
- [x] SessionWeft 0.2.0 exact-commit GA gate

## 0.2.0 GA scope

- SQLite local single-user Runtime mode.
- Authenticated PostgreSQL and NATS JetStream service mode.
- Multi-tenant SaaS identities, memberships, tokens, quotas and isolated Runtime schemas.
- Tenant-scoped Session, Agent, Workflow and Lock APIs with cross-tenant not-found semantics.
- Billing plans, subscriptions, entitlements and append-only usage authority.
- Idempotent Stripe customer, subscription, meter event and raw webhook reference adapter.
- Runtime-owned Session, Workflow, Agent, Memory, Lock, Git, Provider, Tool and event state.
- CLI, Ratatui TUI and VS Code clients as stateless Runtime adapters.
- Official MCP SDK integration and one-time approvals.
- Linux native plugin sandbox using bubblewrap.
- Portable Wasmtime plugin sandbox on Linux, macOS and Windows.
- Exact-commit certification and fail-closed activation for provider, plugin, deployment and billing adapters.
- Revision-aware workspace intelligence for Rust, TypeScript/JavaScript and Python.

## Remaining release constraints

- Deployments that disable authentication, tenant isolation, approval, fencing, durable storage or audit controls are outside the supported scope.
- Native process sandbox guarantees outside Linux are not claimed; non-Linux production plugins use the portable Wasmtime boundary.
- Every future adapter version must pass certification for the exact release commit before activation.
- External payment-provider availability does not override Runtime-owned entitlement state.

## Release guarantees

- Locked Rust dependency graph, rustfmt, Clippy with warnings denied and workspace tests.
- PostgreSQL/JetStream service-mode ownership, redelivery and recovery tests.
- Tenant isolation, quota replay and SaaS Runtime restart persistence tests.
- Billing usage, webhook and Stripe operation idempotence.
- Portable sandbox tests on Ubuntu, macOS and Windows.
- Adapter contract, compatibility, security, recovery, observability and supply-chain certification.
- Durable scheduler claims, stale-Agent handover and external side-effect idempotency.
- Hierarchical lock leases and fencing-token enforcement.
- Isolated Git worktrees, compare-and-swap merge queue and conflict recovery.
- Default-deny Tool/MCP execution with durable approval consumption.
- Backup/restore, migration, provider outage and network-partition drills.
- Secret scanning, dependency audit, SBOM, checksums and provenance attestations.
- Metrics, alerts, dashboard and operations runbooks.

## Approval model

The human repository owner `khovan123` is the approving authority. `sessionweft-automation` performs delegated research and evidence analysis. A release gate cannot pass with a failed or waived required gate, an open Critical or High finding, a non-human required sign-off, or evidence that is not bound to the exact tested commit.

## Release artefacts

- 0.2.0 GA policy: `release/ga-policy-0.2.0.json`
- 0.2.0 GA evidence template: `release/evidence/ga-0.2.0.json`
- 0.2.0 exact-commit workflow: `.github/workflows/ga-0.2.0.yml`
- Phase 3 qualification: `.github/workflows/phase3-qualification.yml`
- SaaS integration: `.github/workflows/saas-runtime.yml`
- Publication: `.github/workflows/publish-v0.2.0.yml`
- Publication verification: `.github/workflows/verify-v0.2.0-publication.yml`
- GA record: `docs/09-release/general-availability-0.2.0.md`
- Contributor avatars: `CONTRIBUTING.md`, maintained by `.github/workflows/contributors.yml`

`v0.1.0` remains immutable and available as the original GA scope.
