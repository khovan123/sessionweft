# Phase -1 Capability Matrix

Status: **Draft baseline**  
Owner: Project Architecture  
Gate: Phase -1 is not complete until every `Must` capability has measurable acceptance criteria and the matrix is reviewed.

## Priority definition

- **Must**: required for the first production-capable release.
- **Should**: important but may be delivered after the first vertical slice.
- **Could**: extension point or optimization; must not distort the core architecture.

## Matrix

| ID | Domain | Capability | Priority | Production / security requirement | Initial acceptance criterion | OSS candidates | Status |
|---|---|---|---|---|---|---|---|
| SES-001 | Session | Create, load, update and archive a session | Must | Durable identifiers; authenticated access | A session survives runtime restart and retains its identity | SQLite, PostgreSQL | Draft |
| SES-002 | Session | Session as the single source of truth | Must | No hidden durable agent/client state | All resumable task, workflow, provider and lock state can be rebuilt from runtime-owned storage | Custom domain model | Draft |
| SES-003 | Session | Optimistic concurrency and version checks | Must | Prevent silent lost updates | Conflicting writes are detected and produce a typed error | SQLite/PostgreSQL transactions | Draft |
| SES-004 | Session | Snapshot and recovery | Must | Crash-safe and auditable | Recovery tests restore the last committed state after forced termination | SQLite WAL, PostgreSQL | Draft |
| SES-005 | Session | Timeline and decision history | Must | Immutable audit metadata | Every state transition has actor, time, correlation and causation identifiers | Event log / DB | Draft |
| SES-006 | Session | Export and import | Should | Secrets and credentials excluded | Exported data can recreate a session in a clean runtime without secret leakage | Custom format | Draft |
| PRV-001 | Provider | Common provider interface | Must | Provider-specific fields isolated | Two unrelated providers pass the same conformance suite | Direct adapters, LiteLLM, OpenRouter | Draft |
| PRV-002 | Provider | Streaming normalized events | Must | Cancellation, timeout and partial failure handling | Text, reasoning metadata and tool calls are represented as typed stream events | OpenAI, Anthropic, Gemini, Ollama | Draft |
| PRV-003 | Provider | Tool calling normalization | Must | JSON schema validation and permission checks | The same runtime tool can be invoked through both reference providers | Provider APIs, MCP | Draft |
| PRV-004 | Provider | Hot-swap provider without losing session context | Must | Provider change is audited | A running session changes provider and continues with equivalent runtime-owned context | Direct adapters / gateway | Draft |
| PRV-005 | Provider | Interrupt, cancel and resume | Must | Bound execution and resource use | Cancellation reaches the provider adapter and terminal state is persisted | Tokio cancellation | Draft |
| PRV-006 | Provider | Usage, cost and rate-limit accounting | Must | No credentials in telemetry | Usage is normalized and attributable to session, task, agent and provider | Direct API metadata, LiteLLM | Draft |
| PRV-007 | Provider | Fallback and retry policy | Should | Idempotency and bounded retries | Eligible transient failures use policy-driven fallback without duplicating tool side effects | LiteLLM inspiration | Draft |
| AGT-001 | Agent | Agent manifest and capability declaration | Must | Least privilege | Runtime rejects undeclared tool, workspace or provider capabilities | Custom SDK | Draft |
| AGT-002 | Agent | Lifecycle: register, start, heartbeat, stop | Must | Stale agents are detected | Agent state transitions are persisted and observable | Custom runtime | Draft |
| AGT-003 | Agent | Task assignment and handover | Must | Ownership is explicit and auditable | A task can move to another agent after failure without losing prior outputs | Session + event bus | Draft |
| AGT-004 | Agent | Planner, architect, worker, reviewer, tester and merger roles | Must | Role permissions separated | A reference workflow completes through all required roles | Custom Agent SDK | Draft |
| AGT-005 | Agent | Failure recovery | Must | No permanent task ownership by a dead process | Forced agent termination triggers lease expiry and resumable reassignment | Heartbeat + leases | Draft |
| WF-001 | Workflow | YAML workflow definition | Must | Strict schema and versioning | Invalid workflows fail before execution with actionable errors | serde_yaml / JSON Schema | Draft |
| WF-002 | Workflow | DAG validation and cycle detection | Must | Deterministic validation | Cyclic graphs are rejected; valid graphs produce stable execution order | petgraph or custom DAG | Draft |
| WF-003 | Workflow | Parallel fan-out and fan-in | Must | Bounded concurrency | Independent worker tasks run concurrently and join at a defined barrier | Tokio tasks | Draft |
| WF-004 | Workflow | Retry, timeout and fallback | Must | Idempotency keys and retry limits | Retry policy does not repeat committed side effects | Custom DAG; Temporal inspiration | Draft |
| WF-005 | Workflow | Conditional branches and approvals | Must | Approval actor is authenticated and audited | Workflow pauses and resumes from a persisted approval decision | Custom DAG | Draft |
| WF-006 | Workflow | Rollback and compensation | Must | Explicit compensation contract | A failed workflow invokes declared compensation steps and records outcome | Saga pattern | Draft |
| WF-007 | Workflow | Durable resume | Must | Restart-safe execution | Runtime restart does not rerun completed nodes | Session persistence | Draft |
| EVT-001 | Event | Versioned event envelope | Must | Correlation, causation, session and actor metadata | All domain events validate against a published schema | Protobuf / serde | Draft |
| EVT-002 | Event | Local Pub/Sub | Must | Bounded queues and backpressure policy | Runtime components exchange typed events without direct ownership coupling | Tokio mpsc/broadcast | Draft |
| EVT-003 | Event | Durable production event transport | Must | Authentication, replay and consumer state | Durable consumers resume after disconnect and replay required events | NATS JetStream | Draft |
| EVT-004 | Event | Idempotent consumption and deduplication | Must | Duplicate delivery cannot corrupt state | Injected duplicate events produce one committed state transition | Event IDs + DB uniqueness | Draft |
| EVT-005 | Event | Dead-letter and retry handling | Must | Poison messages isolated | Failed events are observable and can be replayed after correction | NATS JetStream / custom local adapter | Draft |
| WSP-001 | Workspace | Workspace discovery and graph | Must | Root boundaries enforced | Runtime discovers directories and files without escaping configured roots | walkdir / ignore | Draft |
| WSP-002 | Workspace | File watching and incremental updates | Must | Symlink and path traversal safety | A file change updates only affected index entries and emits `FileChanged` | notify | Draft |
| WSP-003 | Workspace | Syntax and symbol graph | Must | Parser failures isolated | Supported languages expose file, symbol and reference metadata incrementally | tree-sitter, LSP | Draft |
| WSP-004 | Workspace | Text and symbol search | Must | Resource limits | Search returns ranked results under an agreed repository-size benchmark | ripgrep, Tantivy | Draft |
| WSP-005 | Workspace | Shared workspace snapshot | Should | Snapshot provenance | A task records the workspace revision used to build its context | Git + index metadata | Draft |
| LCK-001 | Collaboration | Hierarchical workspace/directory/file/symbol locking | Must | Conflict matrix is deterministic | Conflicting parent/child locks cannot be granted concurrently | Custom lock service | Draft |
| LCK-002 | Collaboration | Lease, heartbeat and timeout | Must | Dead owners cannot retain locks | Killing an agent releases or expires its locks within the configured bound | Runtime leases | Draft |
| LCK-003 | Collaboration | Wait queue and fairness | Should | Starvation prevention | Waiting requests are ordered by documented policy and expose queue state | Custom lock service | Draft |
| LCK-004 | Collaboration | Ownership transfer | Should | Transfer requires authorization | Lock transfer is atomic and appears in the audit timeline | Custom lock service | Draft |
| LCK-005 | Collaboration | Merge queue and conflict resolver contract | Must | Changes cannot bypass policy | Only eligible changes enter merge; conflicts create explicit resolution tasks | Git integration | Draft |
| GIT-001 | Git | Status, diff and revision identity | Must | Repository boundaries enforced | Session records repository identity, base revision and task diff | gitoxide, libgit2 | Draft |
| GIT-002 | Git | Branch or worktree isolation | Must | Agent changes isolated | Concurrent workers do not write to the same uncontrolled working tree | gitoxide/libgit2/Git CLI | Draft |
| GIT-003 | Git | Commit, merge and rollback | Must | Author and task provenance | Merge failure leaves repository recoverable and records the failed attempt | gitoxide, libgit2 | Draft |
| MEM-001 | Memory | Pluggable memory interface | Must | Data ownership and deletion | A memory backend can be replaced without changing session or agent contracts | Mem0, Graphiti, Zep, custom | Draft |
| MEM-002 | Memory | Memory classes and provenance | Must | Source, session, actor and timestamp required | Retrieved memory links to the source material that produced it | Custom metadata | Draft |
| MEM-003 | Memory | Retrieval, ranking and deduplication | Must | Query isolation and bounded cost | Benchmark reports relevance, latency and token contribution | Qdrant, BM25, graph retrieval | Draft |
| MEM-004 | Memory | Retention, deletion and privacy filtering | Must | Secrets excluded; deletion verifiable | Deleted or expired memory is absent from subsequent retrieval | Backend-specific | Draft |
| MEM-005 | Memory | Temporal and decision memory | Should | Conflicting facts retain validity history | Queries can distinguish current decisions from superseded decisions | Graphiti / temporal model | Draft |
| CTX-001 | Context | Incremental context assembly | Must | Full history is not broadcast by default | Context is built from task, dependencies, summaries, files, memory and locks | Custom engine | Draft |
| CTX-002 | Context | Token budget and compression | Must | Deterministic priority rules | Context never exceeds provider budget and records omitted sections | Tokenizers + summaries | Draft |
| CTX-003 | Context | Relevance explanation | Must | Auditability | Every included item records source and inclusion reason | Custom engine | Draft |
| CTX-004 | Context | Provider-aware formatting | Must | Preserve semantic requirements | Equivalent context is encoded according to provider capability constraints | Provider SDK | Draft |
| MCP-001 | MCP | MCP client and server support | Must | Capability negotiation and transport security | Runtime interoperates with official MCP conformance examples | Official Rust SDK (`rmcp`) | Draft |
| MCP-002 | MCP | Tool discovery and invocation | Must | Schema validation and least privilege | Discovered tools cannot execute without runtime permission evaluation | `rmcp` | Draft |
| MCP-003 | MCP | Cancellation, timeout and audit | Must | Every call bounded and attributable | Tool calls can be cancelled and produce a terminal audit record | `rmcp` + runtime policy | Draft |
| PLG-001 | Plugin | Plugin manifest, version and lifecycle | Must | Compatibility checks | Incompatible plugins fail before activation without crashing runtime | Custom Plugin SDK | Draft |
| PLG-002 | Plugin | Permission model and isolation | Must | Default deny | A malicious test plugin cannot read undeclared secrets or paths | Process/WASM isolation TBD | Draft |
| API-001 | API | gRPC/Protobuf client contract | Must | Authentication, TLS and versioning | CLI and IDE perform all durable operations through versioned services | tonic, prost | Draft |
| API-002 | API | Unary and bidirectional streaming | Must | Backpressure, reconnect and cancellation | Long-running execution streams progress and can reconnect/resume by ID | tonic | Draft |
| CLI-001 | Client | Headless CLI | Must | No direct DB/provider access | CLI creates, opens, resumes and inspects sessions through runtime APIs | clap | Draft |
| CLI-002 | Client | Interactive TUI | Should | Same API boundary as CLI | TUI displays session, task, agent, workflow, lock and event status | Ratatui | Draft |
| IDE-001 | Client | VS Code extension | Must | Runtime remains alive when IDE closes | Extension reconnects to an existing session and restores views | VS Code API | Draft |
| SEC-001 | Security | Authentication and session authorization | Must | Explicit identity and access policy | Unauthorized clients cannot list or mutate sessions | TLS/OAuth/token TBD | Draft |
| SEC-002 | Security | Secret management and redaction | Must | Secrets never enter normal logs/events/memory | Automated tests inject secrets and verify redaction paths | Secret store TBD | Draft |
| SEC-003 | Security | Tool and plugin policy enforcement | Must | Default deny and auditable approval | High-risk actions require policy approval and produce audit entries | Custom policy engine | Draft |
| OBS-001 | Observability | Structured logs, metrics and traces | Must | Correlation across session/task/agent/provider | One execution can be followed end-to-end by correlation ID | tracing, OpenTelemetry | Draft |
| OBS-002 | Observability | Cost, latency, failure and lock metrics | Must | Labels must avoid sensitive/high-cardinality data | Dashboards expose agreed service and product indicators | OpenTelemetry/Prometheus TBD | Draft |
| DEP-001 | Deployment | Local single-node mode | Must | Minimal external dependencies | A documented command starts runtime with local storage and test provider | SQLite + local event adapter | Draft |
| DEP-002 | Deployment | Production service mode | Must | Durable storage, secure transport and recovery | Staging validates restart, backup, restore and rolling upgrade | PostgreSQL + NATS + optional Qdrant | Draft |
| TST-001 | Testing | Contract and conformance suites | Must | Repeatable in CI | Provider, event, RPC, plugin and MCP contracts run without external paid services | Mocks + official test suites | Draft |
| TST-002 | Testing | Recovery and chaos tests | Must | Forced failures included | Runtime, agent, transport and DB failures preserve documented invariants | Test harness | Draft |
| TST-003 | Testing | Performance benchmarks | Must | Reproducible datasets and hardware metadata | Baselines exist for session load, events, context and repository indexing | Criterion + scenario harness | Draft |

## Phase -1 open questions

1. Which capabilities require explicit numeric SLOs before Phase 0 scoring?
2. Which languages are required for the first workspace indexing benchmark?
3. Is multi-user tenancy required for the first production release or only single-user local plus authenticated service mode?
4. Which tool actions require mandatory human approval?
5. Which session data must be portable across runtime versions?
6. What is the minimum offline/local-model experience?

## Review checklist

- [ ] Every domain in `PROJECT.md` is represented.
- [ ] Every `Must` capability has an observable acceptance criterion.
- [ ] Security requirements are explicit for external boundaries.
- [ ] Recovery requirements cover process and infrastructure failures.
- [ ] OSS candidates are treated as research targets, not approved dependencies.
- [ ] Product owner and architecture reviewer approve the Phase -1 baseline.
