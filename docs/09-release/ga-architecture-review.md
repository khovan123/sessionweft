# Architecture GA Review — SessionWeft 0.1.0

Decision: **Approved for General Availability within the declared scope**.  
Approving authority: `khovan123`  
Analysis executor: `sessionweft-automation`  
Review date: 2026-07-22

## Review basis

The review evaluated the merged architecture, ADR/RFC set, implementation boundaries, recovery semantics and the release evidence from commit `c0aa35a5593bee306a26d1b4ff7c63b5ba26a722` and its GA approval successor.

External reference baseline:

- NIST Secure Software Development Framework 1.1: https://csrc.nist.gov/pubs/sp/800/218/final
- NIST SSDF 1.2 initial public draft, reviewed for newer guidance without treating the draft as a mandatory standard: https://csrc.nist.gov/pubs/sp/800/218/r1/ipd
- SLSA Build track 1.2: https://slsa.dev/spec/v1.2/build-track-basics
- GitHub artifact attestations: https://docs.github.com/en/actions/concepts/security/artifact-attestations

## Architecture evidence reviewed

- Runtime is the only durable owner of Session, Workflow, Agent, Memory, Lock, Git, Tool and event state.
- HTTP, CLI, TUI and VS Code remain adapters and do not access repositories directly.
- SQLite local mode and PostgreSQL/JetStream service mode implement the same domain contracts.
- Session, Workflow and Agent state use explicit versions and optimistic concurrency.
- State mutations and Outbox events are committed atomically.
- Scheduler task claims are durable, idempotent and recoverable after stale Agent detection.
- Hierarchical lock leases and fencing tokens protect shared workspace mutations.
- Provider and Tool side effects use a persisted execution ledger with an explicit `uncertain` state instead of blind retry.
- Git workers use isolated worktrees, fenced commits and compare-and-swap target updates.
- MCP transport, plugins and model providers cannot own Session state or bypass Runtime authorization.
- Workspace intelligence uses revision-aware indexes and rebuildable projections rather than making an index the source of truth.
- Client event cursors and Runtime-owned PTYs survive client disconnects.

## Findings

- No unresolved architecture violation blocks GA within the declared scope.
- Ownership and recovery boundaries are explicit and covered by restart, contention, chaos and integration tests.
- Local mode remains usable without PostgreSQL or NATS.
- Service mode supports two Runtime instances without duplicate task or exclusive lock ownership in the tested profile.
- Deployment adapters outside the approved scope must repeat architecture and release qualification before being represented as GA.

## Residual architecture risks

- Multi-tenant isolation is not implemented and is excluded from GA.
- The Linux bubblewrap sandbox is the production plugin-isolation baseline; other operating systems do not inherit that production claim.
- Capacity targets are release limits, not unlimited scalability guarantees.
- A future distributed lock authority, provider adapter or workflow engine replacement requires a new ADR and compatibility gate.

## Approval

Architecture is approved for SessionWeft 0.1.0 GA within the scope recorded in `ga-authorization.md`. This approval is invalid if a required technical gate fails or if the exact tested commit cannot be identified.
