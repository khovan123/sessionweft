# Project Status

Last updated: 2026-07-23  
Current phase: **General Availability — SessionWeft 0.2.0**  
Decision: **Approved for GA when every exact-commit gate passes**

## Completed phases

- [x] Phase -1 — Capability Matrix
- [x] Phase 0 — Landscape Research
- [x] Architecture Review
- [x] ADR baseline
- [x] RFC and Production Specification
- [x] Phase 2 implementation
- [x] Production testing and chaos/recovery qualification
- [x] SessionWeft 0.1.0 General Availability
- [x] Phase 3 tenant and billing authority
- [x] Portable Wasmtime plugin sandbox qualification
- [x] Exact-commit adapter certification and Runtime activation
- [x] SessionWeft 0.2.0 General Availability gate

## 0.2.0 GA scope

- SQLite local single-user Runtime mode.
- Authenticated single-tenant service mode using PostgreSQL and NATS JetStream.
- Multi-tenant SaaS Runtime using isolated PostgreSQL schemas.
- Tenant-owned identity, membership, quota and API-token authority.
- Tenant-scoped Session, Agent, Workflow and Lock APIs.
- Billing plans, subscriptions, entitlements, usage records and idempotent Stripe integration.
- Runtime-owned Session, Workflow, Agent, Memory, Lock, Git, Provider, Tool and event state.
- CLI, Ratatui TUI and VS Code clients as stateless Runtime adapters.
- Official MCP SDK integration and one-time approvals.
- Linux native plugin isolation using bubblewrap.
- Portable Wasmtime/WASI plugin isolation on Linux, macOS and Windows.
- Exact-commit certification and fail-closed activation for production provider, plugin, deployment and billing adapters.

## Remaining exclusions

- Deployments that disable required authentication, approval, fencing, durable storage, tenant isolation or audit controls.
- Production adapters that are absent from the packaged activation policy or do not have a matching exact-commit certification.
- Plugin filesystem or network access that has not been explicitly granted through the declared capability boundary.
- External services or providers outside their documented adapter contracts and operational limits.

## Release guarantees

- Locked Rust dependency graph, rustfmt, Clippy with warnings denied and workspace tests.
- PostgreSQL and JetStream service-mode ownership, redelivery and recovery tests.
- Durable scheduler claims, stale-Agent handover and external side-effect idempotency.
- Hierarchical lock leases and fencing-token enforcement.
- Isolated Git worktrees, compare-and-swap merge queue and conflict recovery.
- Tenant Runtime restart persistence and cross-tenant not-found behavior.
- Billing usage idempotence and tenant-scoped entitlement authority.
- Portable sandbox qualification on Linux, macOS and Windows.
- Exact adapter ID, version, kind and build-commit activation checks.
- Backup/restore, migration, provider outage and network-partition drills.
- Secret scanning, dependency audit, SBOM, checksums and provenance attestations.
- Metrics, alerts, dashboard and operations runbooks.

## GA approval model

The human repository owner `khovan123` is the approving authority. `sessionweft-automation` performs delegated research, implementation support and evidence analysis. The 0.2.0 approval is valid only when the exact main-branch commit passes CI, security, production hardening, SaaS Runtime, Phase 3 qualification and GA approval before `v0.2.0` is created.

The GA gate cannot pass when:

- required evidence identifies `TBD` instead of the tested commit;
- a required gate is failed or waived;
- a Critical or High security finding remains open;
- Architecture, Security or Operations GA approval is missing;
- the approving authority is not recorded as human;
- release evidence has no supporting artefacts;
- an activated adapter does not match its certification by ID, version, kind and exact build commit.

## Release artefacts

- GA policy: `release/ga-policy-0.2.0.json`
- GA evidence template: `release/evidence/ga-0.2.0.json`
- Adapter activation policy: `release/adapters/activation.json`
- Verified adapter certifications: `release/adapters/verified/*.json`
- Phase 3 workflow: `.github/workflows/phase3-qualification.yml`
- SaaS Runtime workflow: `.github/workflows/saas-runtime.yml`
- GA workflow: `.github/workflows/ga-approval.yml`
- Tag authorization workflow: `.github/workflows/publish-v0.2.0-tag.yml`
- Release workflow: `.github/workflows/release.yml`
- Publication verification workflow: `.github/workflows/verify-v0.2.0-publication.yml`
- GA record: `docs/09-release/general-availability-0.2.0.md`
