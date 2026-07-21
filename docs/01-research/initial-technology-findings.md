# Initial Technology Findings

Status: **Research notes — not approved architecture**  
Date: 2026-07-22  
Purpose: establish the first evidence-backed shortlist and identify decisions that require ADRs.

## Executive recommendation

Keep the planned Rust/Tokio runtime and gRPC/Protobuf API boundary. Build runtime-owned interfaces around event transport, provider adapters, memory, Git and MCP so external systems remain replaceable. The first implementation slice should prove session durability and provider hot-swap before multi-agent feature breadth.

## Findings

### 1. Rust async runtime: retain Tokio

**Finding**

Tokio provides the async I/O, scheduling, synchronization, channels and shutdown primitives required by the runtime. Its channel types support both bounded multi-producer/single-consumer command paths and multi-consumer broadcast paths.

**Recommendation**

Adopt Tokio as the runtime foundation, subject to ADR approval. Use bounded channels by default and document overflow/backpressure behavior. Do not treat in-memory channels as durable events.

**Primary sources**

- https://tokio.rs/
- https://tokio.rs/tokio/tutorial/channels
- https://tokio.rs/tokio/topics/shutdown

---

### 2. RPC: retain gRPC/Protobuf through tonic/prost

**Finding**

`tonic` is a Tokio-based Rust gRPC implementation with unary and bidirectional streaming, TLS, authentication hooks, health checking and Protobuf code generation through `prost`.

**Recommendation**

Adopt `tonic` and `prost` for the runtime/client protocol. Pin a released version rather than the repository default branch, and define compatibility rules before implementation.

**Primary sources**

- https://github.com/hyperium/tonic
- https://docs.rs/tonic/latest/tonic/

---

### 3. Events: separate local delivery from durable event transport

**Finding**

Tokio channels are suitable for local process coordination but do not provide durable replay. NATS JetStream provides persisted streams, durable/ephemeral consumers, acknowledgements and replay.

**Recommendation**

Create an `EventTransport` interface with two implementations:

1. `LocalEventTransport` for development and in-process notifications.
2. `NatsJetStreamTransport` for durable production delivery.

Durable domain transitions must be committed to session storage before or atomically with publication using an outbox-style design. Core NATS or Tokio broadcast alone must not be the source of truth.

**ADR required**

- Local versus durable event semantics.
- Outbox and replay policy.
- Delivery guarantee: design for at-least-once plus idempotent consumers.

**Primary sources**

- https://docs.nats.io/nats-concepts/jetstream
- https://docs.nats.io/nats-concepts/jetstream/consumers
- https://tokio.rs/tokio/tutorial/channels

---

### 4. MCP: shortlist the official Rust SDK

**Finding**

The Model Context Protocol organization maintains an official Rust SDK (`rmcp`) using Tokio. It supports clients, servers, tools, resources, prompts, logging, subscriptions, cancellation and multiple transports.

**Recommendation**

Adopt the official Rust SDK as the primary MCP candidate. Wrap it behind SessionWeft-owned permission, timeout, audit and lifecycle interfaces. Do not expose SDK objects directly in core domain models.

**Primary sources**

- https://github.com/modelcontextprotocol/rust-sdk
- https://github.com/modelcontextprotocol/modelcontextprotocol

---

### 5. Provider strategy: direct conformance first, gateway optional

**Finding**

Provider APIs expose overlapping but non-identical concepts for streaming, tool calls, usage and server-managed state. OpenAI, Gemini and Ollama all provide streaming and tool/function calling, but event shapes and continuation rules differ. LiteLLM provides broad provider routing, cost tracking, load balancing and logging as an external Python gateway.

**Recommendation**

- Build a Rust-native `Provider` contract owned by SessionWeft.
- Use **OpenAI and Ollama** as the first reference adapters: one hosted provider and one local provider create a useful conformance boundary.
- Add Anthropic and Gemini after the interface passes the first conformance suite.
- Support LiteLLM as an optional gateway adapter, not a mandatory runtime dependency.
- Persist normalized runtime context and tool state; never rely on a provider's conversation identifier as the session source of truth.

**Primary sources**

- https://platform.openai.com/docs/api-reference/responses
- https://ai.google.dev/api
- https://ai.google.dev/gemini-api/docs/function-calling
- https://docs.ollama.com/capabilities/tool-calling
- https://docs.ollama.com/capabilities/streaming
- https://github.com/BerriAI/litellm

---

### 6. Memory: interface first; benchmark before adopting a memory platform

**Finding**

Mem0 provides a general agent memory layer with session/agent/user concepts and multiple retrieval signals. Graphiti provides temporal knowledge graphs with provenance and hybrid semantic, keyword and graph retrieval. Both add non-Rust services and operational dependencies when self-hosted.

**Recommendation**

- Define a `MemoryProvider` interface and provenance model first.
- Implement a deterministic baseline using runtime storage plus lexical/vector retrieval.
- Benchmark Mem0 and Graphiti against SessionWeft's Conversation, Repository, Decision, Preference and Error memory classes.
- Do not make either service mandatory until retrieval quality, deletion, isolation, latency and operating cost are measured.
- Treat temporal decision memory as a distinct benchmark because superseded architecture decisions must remain queryable without appearing current.

**Primary sources**

- https://github.com/mem0ai/mem0
- https://github.com/getzep/graphiti
- https://github.com/getzep/zep

---

### 7. Workflow: preserve the custom DSL requirement, challenge the custom durability layer

**Finding**

Temporal provides durable, crash-resumable workflow execution. SessionWeft requires a YAML/DAG representation tightly integrated with session state, agent roles, workspace locks, provider calls, approvals and merge operations.

**Recommendation**

Research two alternatives before ADR approval:

- **A — Custom DAG scheduler and durability:** maximum domain control, highest correctness burden.
- **B — SessionWeft DSL compiled to Temporal workflows:** mature durability, additional service and semantic constraints.

Do not begin scheduler implementation until replay determinism, idempotency, rollback and upgrade behavior are compared through prototypes.

**Primary source**

- https://docs.temporal.io/

---

### 8. Git: keep gitoxide and libgit2 in the shortlist

**Finding**

`gitoxide` is an actively maintained pure-Rust Git implementation. The project still needs a capability-level comparison for status, diff, worktree, merge, conflict resolution, performance and API stability.

**Recommendation**

Prototype both `gitoxide` and a libgit2-based option against the required operations. The initial implementation may use a restricted Git CLI adapter only as a test oracle, not as an ungoverned shell escape.

**Primary source**

- https://github.com/GitoxideLabs/gitoxide

---

## Provisional decisions to convert into ADRs

| Topic | Provisional recommendation | Confidence | ADR |
|---|---|---:|---|
| Runtime | Rust + Tokio | High | ADR-001 |
| Client RPC | tonic + prost over gRPC | High | ADR-002 |
| Local events | Bounded Tokio channels behind an interface | Medium | ADR-003 |
| Durable events | NATS JetStream adapter with idempotent consumers | High | ADR-003 |
| MCP | Official Rust SDK wrapped by runtime policy | High | ADR-015 |
| Provider references | OpenAI + Ollama first; Anthropic/Gemini next | Medium | ADR-007 |
| Gateway | LiteLLM optional, never core-owned state | Medium | ADR-008 |
| Memory | Interface + baseline first; Mem0/Graphiti benchmark | High | ADR-011 |
| Workflow durability | Prototype custom versus Temporal-backed execution | Low | ADR-010 |
| Git library | Benchmark gitoxide versus libgit2 | Low | ADR-014 |

## First technical spike order

1. Session state model and crash-recovery invariant.
2. Provider normalized stream and tool-call model.
3. OpenAI/Ollama conformance adapters.
4. Local event transport plus durable outbox model.
5. NATS JetStream replay/idempotency spike.
6. MCP official SDK interoperability spike.
7. Workflow durability comparison.
8. Git operation comparison.
9. Memory retrieval benchmark.

No spike result becomes architecture until its evidence and consequences are recorded in an ADR.
