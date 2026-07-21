# Project Status

Last updated: 2026-07-22  
Current phase: **Phase 2 — First Runtime Vertical Slice**  
Implementation status: **Authorized by Architecture/ADR/RFC baseline**

## Completed gates

### Phase -1 — Capability Matrix

- [x] Project domains mapped to capability IDs
- [x] Must/Should/Could priorities defined
- [x] Observable acceptance criteria defined
- [x] Coding Agent, Terminal and Vector Store gaps corrected
- [x] Security, recovery, deployment and compatibility reviewed
- [x] Capability Matrix approved

### Phase 0 — Landscape Research baseline

- [x] State, event, provider and security foundations researched
- [x] Wave B/C candidates assigned dispositions
- [x] Runtime-owned state invariant preserved
- [x] Replacement boundaries defined for external systems
- [x] Technology shortlist recorded in `phase-0-synthesis.md`
- [x] First-slice scope and deferred work separated

### Architecture, ADR and RFC baseline

- [x] Architecture baseline v1
- [x] ADR-0001 Session persistence
- [x] ADR-0002 Event transport and outbox
- [x] ADR-0003 Provider contract
- [x] ADR-0004 Security and observability
- [x] RFC-0001 Runtime vertical slice
- [x] Production Specification v0

These approvals authorize the first constrained implementation slice. They do not declare the platform generally available.

## Active implementation scope

1. Rust workspace and CI.
2. Versioned Session aggregate.
3. SQLite WAL repository.
4. Transactional outbox.
5. Bounded local event transport.
6. Provider registry.
7. Echo and Ollama-compatible providers.
8. Runtime orchestration service.
9. Bootstrap HTTP control API and CLI.
10. Authentication, correlation, redaction and structured logs.
11. Concurrency, persistence and recovery tests.

## Deferred implementation streams

| Issue | Stream | Current decision |
|---|---|---|
| #4 | Memory engines | Runtime-owned typed baseline first; adapters later |
| #5 | Workflow durability | Persisted Runtime DAG planned after core Session slice |
| #6 | Git/workspace isolation | Restricted Git CLI adapter first |
| #7 | MCP/plugin isolation | Official Rust SDK behind policy gate |
| #9 | Workspace parsing/indexing | tree-sitter + ripgrep baseline |
| #13 | Coding-agent architecture | Runtime roles; no isolated durable CLI sessions |
| #14 | Locking and leases | Persisted hierarchical leases with fencing |
| #15 | CLI/TUI/IDE | CLI first; TUI and IDE after stable API |
| #16 | Vector storage | Optional projection; pgvector before mandatory Qdrant |

## Current approved direction

- Rust 1.88+ and Tokio.
- Runtime-owned Session identity and state.
- SQLite WAL local mode and PostgreSQL service mode.
- Optimistic concurrency with expected versions.
- Transactional outbox.
- Local bounded event adapter and NATS JetStream production adapter.
- Direct provider adapters behind a common contract.
- Echo and Ollama-compatible reference providers.
- Default-deny external execution.
- Structured telemetry without request-body logging.

## Implementation gate

A code PR may merge only when formatting, lint, unit/integration tests, concurrency tests, recovery tests, authentication tests and documentation checks pass.
