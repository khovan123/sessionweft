# Project Status

Last updated: 2026-07-22  
Current phase: **Phase 0 — Landscape Research**  
Implementation status: **Blocked until Architecture, ADR and RFC gates**

## Completed gates

### Phase -1 — Capability Matrix

- [x] Project domains mapped to capability IDs
- [x] Must/Should/Could priorities defined
- [x] Observable baseline acceptance criteria defined
- [x] Coding Agent, Terminal and Vector Store gaps corrected
- [x] Security, recovery, deployment and compatibility capabilities reviewed
- [x] Product-level Phase 0 scope decisions recorded
- [x] Capability Matrix approved for research

Phase -1 approval authorizes research and prototypes only. It does not approve production architecture or dependencies.

## Phase 0 active work

| Issue | Research stream | Status |
|---|---|---|
| #2 | Provider API and routing | Ready |
| #3 | Local events, JetStream and outbox | Ready |
| #4 | Memory engines and retrieval benchmark | Ready |
| #5 | Workflow durability | Ready |
| #6 | Git and workspace isolation | Ready |
| #7 | MCP and plugin isolation | Ready |
| #8 | Session persistence and crash recovery | Ready |
| #9 | Workspace parsing and indexing | Ready |

## Phase 0 additional reports required by `PROJECT.md`

- Coding agent landscape
- Locking and lease models
- CLI, TUI and terminal architecture
- VS Code extension architecture
- Vector database comparison
- License compatibility matrix
- Security and maintenance risk matrix
- Reuse scorecard
- Implementation effort estimate
- Phase 0 approval record

## Current provisional direction

- Rust + Tokio runtime
- tonic/prost gRPC boundary
- Runtime-owned session state
- Local event adapter plus NATS JetStream durable adapter
- Official MCP Rust SDK behind SessionWeft policy wrappers
- Direct provider conformance before optional gateways
- Memory provider interface before adopting a memory platform
- Prototype workflow durability alternatives before implementation

These remain research recommendations until Architecture Review and ADR approval.

## Product scope fixed for research

- Initial workspace languages: Rust, TypeScript/JavaScript and Python.
- Deployment: local single-user plus authenticated single-tenant team service.
- Multi-tenant SaaS: outside the first production release.
- Offline baseline: Runtime, CLI, SQLite, local event transport and Ollama-compatible provider.
- High-risk operations require policy approval: destructive file/Git actions, secret access, external side effects, privilege expansion and policy changes.

## Next gate

Phase 0 exits only when every research category has:

- primary-source evidence;
- architecture, license, maintenance, security and production-readiness assessment;
- reproducible prototype or benchmark where material;
- Adopt/Wrap/Fork/Replace/Reject recommendation;
- reuse score and effort estimate;
- documented risks;
- review approval sufficient to begin Architecture Review.
