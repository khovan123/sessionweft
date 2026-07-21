# Phase 0 Research Synthesis

Status: **Approved baseline for Architecture Review**  
Date: 2026-07-22  
Scope: SessionWeft first production-capable vertical slice

## 1. Decision method

The synthesis applies the scorecard in `phase-0-execution-plan.md`. A technology is not selected only because it is popular. The selected design must preserve these invariants:

1. Runtime owns durable state.
2. Session identity is independent of providers and clients.
3. Durable state changes and outbox records commit atomically.
4. Delivery is assumed to be at least once; consumers are idempotent.
5. Tools, plugins and terminal execution are default-deny.
6. Search, memory and vector indexes are rebuildable projections, not sources of truth.

Primary references used for this baseline:

- SQLite WAL: https://sqlite.org/wal.html
- SQLite transactions: https://sqlite.org/lang_transaction.html
- PostgreSQL transaction isolation: https://www.postgresql.org/docs/current/transaction-iso.html
- NATS JetStream consumers: https://docs.nats.io/nats-concepts/jetstream/consumers
- NATS JetStream semantics: https://docs.nats.io/nats-concepts/jetstream
- Tonic: https://docs.rs/tonic/latest/tonic/
- OpenAI Responses streaming: https://platform.openai.com/docs/api-reference/responses-streaming
- Anthropic tool use: https://platform.claude.com/docs/en/agents-and-tools/tool-use/how-tool-use-works
- Gemini function calling: https://ai.google.dev/gemini-api/docs/function-calling
- Ollama chat API: https://docs.ollama.com/api/chat
- MCP specification: https://modelcontextprotocol.io/specification/2025-11-25
- Official MCP Rust SDK: https://github.com/modelcontextprotocol/rust-sdk
- gitoxide: https://github.com/GitoxideLabs/gitoxide

## 2. Final dispositions

| Area | Decision | Disposition | Rationale |
|---|---|---|---|
| Runtime | Rust + Tokio | Adopt | Strong async ecosystem, cancellation and service composition |
| Public API | gRPC + Protobuf using tonic/prost | Adopt | Versioned contracts and streaming support |
| Local database | SQLite WAL | Adopt | Simple local deployment and crash-safe transactions; one-writer limit is acceptable for local mode |
| Service database | PostgreSQL | Adopt | Strong concurrency, isolation and operational tooling |
| Concurrency | Aggregate version column + compare-and-swap update | Adopt | Portable optimistic concurrency across SQLite and PostgreSQL |
| Event publication | Transactional outbox | Adopt | Prevents committed state without a recoverable publication record |
| In-process events | Bounded Tokio channels | Adopt | Low-latency local notifications; not durable |
| Production events | NATS JetStream pull consumers | Wrap | Durable at-least-once transport behind `EventTransport` |
| Provider integration | Direct adapters behind `Provider` trait | Adopt | Keeps provider-specific state outside the Session model |
| Provider gateway | LiteLLM/OpenRouter-style gateway | Optional/Wrap | Operational option, not the core contract |
| Reference providers | Echo test provider + Ollama-compatible adapter | Adopt | Deterministic tests and offline baseline |
| Workflow | Runtime-owned persisted DAG | Adopt for v0 | Required invariants are small enough for first slice; external orchestrator remains replaceable |
| Temporal | Continue research | Defer | Potential service and SDK coupling is not justified for the first vertical slice |
| Locking | Persisted hierarchical leases with fencing tokens | Adopt | Prevents stale workers from committing after lease loss |
| Git | Restricted Git CLI adapter first | Wrap | Git CLI is the correctness oracle and reduces early library risk |
| gitoxide | Continue research | Defer | Candidate for pure-Rust replacement after conformance corpus exists |
| Workspace parsing | tree-sitter + ripgrep | Adopt | Incremental syntax and reliable lexical search |
| LSP | Optional adapter | Wrap | Valuable for semantic data but language-server lifecycle must be isolated |
| Tantivy | Defer | Continue research | Not required before lexical and symbol baselines are measured |
| Memory | Runtime-owned typed memory records + lexical baseline | Adopt | Deterministic provenance, deletion and recovery |
| Mem0/Graphiti/Zep | Adapter candidates | Continue research | No external memory platform may own authoritative Session data |
| Vector store | Replaceable interface; no mandatory local dependency | Adopt boundary | Local mode must remain lightweight |
| Qdrant | Optional service-mode adapter | Wrap | Strong filtering and vector operations, but separate operations burden |
| pgvector | Preferred first service-mode vector adapter | Adopt when needed | Reuses PostgreSQL lifecycle and transaction boundaries |
| MCP | Official Rust SDK behind Runtime policy | Wrap | Interoperability without delegating authorization |
| Plugin isolation | Separate process first; WASM later | Adopt/Defer | Process boundary is implementable now; WASM capability model needs a later ADR |
| CLI | clap client | Adopt | Scriptable control surface with stable JSON output |
| TUI | Ratatui | Defer after core API | Client only, no durable state |
| IDE | VS Code extension | Defer after core API | Reconnects to Runtime; never owns Session state |
| Observability | tracing + OpenTelemetry-compatible fields | Adopt | Correlation across session/task/provider/event |
| Authentication | Local loopback without token; service mode bearer token initially | Adopt for v0 | Minimal viable boundary; OAuth/OIDC remains a service deployment extension |
| Supply chain | Locked dependencies, audit, SBOM and signed releases | Adopt | Required before release |

## 3. Storage and recovery model

- A Session is stored as a versioned aggregate.
- Each mutation supplies `expected_version`.
- The database updates only when the stored version matches.
- Domain events are inserted into the outbox in the same transaction.
- The outbox publisher may publish an event more than once after a crash.
- Event IDs are stable and every durable consumer must deduplicate.
- SQLite WAL is used only on a local filesystem. It is not placed on a network filesystem.
- PostgreSQL service mode retries serialization or optimistic conflicts using bounded policies.

## 4. Provider contract model

The normalized provider boundary contains:

- runtime-owned input messages;
- runtime-owned tool declarations;
- typed stream events for text, tool-call deltas, completion, usage and errors;
- provider request IDs as optional metadata;
- cancellation and timeout;
- normalized usage and cost fields.

Provider conversation or response IDs are never Session identifiers. Server-side storage features may be disabled where supported. A provider adapter is responsible for rebuilding a request from Runtime-owned context.

## 5. Security baseline

- Default deny for tools, plugins, terminal and filesystem writes.
- Every external action has actor, session, task, correlation and policy-decision metadata.
- Request and response bodies are not logged by default.
- Secret values are redacted before logs, events and memory writes.
- MCP discovery does not imply permission to invoke.
- Destructive Git/filesystem actions and secret access require explicit approval policy.
- Local mode binds to loopback by default.
- Service mode refuses to start without an API token until stronger identity integration is configured.

## 6. Architecture gate result

Phase 0 is approved for Architecture Review and the first constrained vertical slice. The approval does not claim GA readiness. Deferred capabilities remain tracked and must not be silently implemented as authoritative dependencies.

The first implementation slice is limited to:

1. Session create/load/update/archive.
2. Optimistic concurrency.
3. SQLite WAL repository.
4. Transactional outbox.
5. Local event delivery.
6. Provider registry with Echo and Ollama-compatible adapters.
7. Runtime service API.
8. CLI client.
9. Structured logs, request correlation and secret redaction.
10. Unit, integration and recovery-oriented tests.
