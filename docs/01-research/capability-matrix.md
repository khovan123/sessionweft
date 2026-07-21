# Phase -1 Capability Matrix

Status: **Approved for Phase 0 — Baseline v1.0**  
Owner: Project Architecture  
Reviewed: 2026-07-22  
Gate: this baseline authorizes landscape research and prototypes only; it does not approve production architecture or dependencies.

## Priority definition

- **Must**: required for the first production-capable release.
- **Should**: important but may be delivered after the first vertical slice.
- **Could**: extension point or optimization; must not distort the core architecture.

## Matrix

| ID | Domain | Capability | Priority | Production / security requirement | Initial acceptance criterion | OSS candidates | Status |
|---|---|---|---|---|---|---|---|
| SES-001 | Session | Create, load, update and archive a session | Must | Durable identifiers; authenticated access | A session survives runtime restart and retains its identity | SQLite, PostgreSQL | Reviewed |
| SES-002 | Session | Session as the single source of truth | Must | No hidden durable agent/client state | All resumable task, workflow, provider and lock state can be rebuilt from runtime-owned storage | Custom domain model | Reviewed |
| SES-003 | Session | Optimistic concurrency and version checks | Must | Prevent silent lost updates | Conflicting writes are detected and produce a typed error | SQLite/PostgreSQL transactions | Reviewed |
| SES-004 | Session | Snapshot and recovery | Must | Crash-safe and auditable | Recovery tests restore the last committed state after forced termination | SQLite WAL, PostgreSQL | Reviewed |
| SES-005 | Session | Timeline and decision history | Must | Immutable audit metadata | Every state transition has actor, time, correlation and causation identifiers | Event log / DB | Reviewed |
| SES-006 | Session | Export and import | Should | Secrets and credentials excluded | Exported data can recreate a session in a clean runtime without secret leakage | Custom format | Reviewed |
| PRV-001 | Provider | Common provider interface | Must | Provider-specific fields isolated | Two unrelated providers pass the same conformance suite | Direct adapters, LiteLLM, OpenRouter | Reviewed |
| PRV-002 | Provider | Streaming normalized events | Must | Cancellation, timeout and partial failure handling | Text, reasoning metadata and tool calls are represented as typed stream events | OpenAI, Anthropic, Gemini, Ollama | Reviewed |
| PRV-003 | Provider | Tool calling normalization | Must | JSON schema validation and permission checks | The same runtime tool can be invoked through both reference providers | Provider APIs, MCP | Reviewed |
| PRV-004 | Provider | Hot-swap provider without losing session context | Must | Provider change is audited | A running session changes provider and continues with equivalent runtime-owned context | Direct adapters / gateway | Reviewed |
| PRV-005 | Provider | Interrupt, cancel and resume | Must | Bound execution and resource use | Cancellation reaches the provider adapter and terminal state is persisted | Tokio cancellation | Reviewed |
| PRV-006 | Provider | Usage, cost and rate-limit accounting | Must | No credentials in telemetry | Usage is normalized and attributable to session, task, agent and provider | Direct API metadata, LiteLLM | Reviewed |
| PRV-007 | Provider | Fallback and retry policy | Should | Idempotency and bounded retries | Eligible transient failures use policy-driven fallback without duplicating tool side effects | LiteLLM inspiration | Reviewed |
| AGT-001 | Agent | Agent manifest and capability declaration | Must | Least privilege | Runtime rejects undeclared tool, workspace or provider capabilities | Custom SDK | Reviewed |
| AGT-002 | Agent | Lifecycle: register, start, heartbeat, stop | Must | Stale agents are detected | Agent state transitions are persisted and observable | Custom runtime | Reviewed |
| AGT-003 | Agent | Task assignment and handover | Must | Ownership is explicit and auditable | A task can move to another agent after failure without losing prior outputs | Session + event bus | Reviewed |
| AGT-004 | Agent | Planner, architect, worker, reviewer, tester and merger roles | Must | Role permissions separated | A reference workflow completes through all required roles | Custom Agent SDK | Reviewed |
| AGT-005 | Agent | Failure recovery | Must | No permanent task ownership by a dead process | Forced agent termination triggers lease expiry and resumable reassignment | Heartbeat + leases | Reviewed |
| COD-001 | Coding Agent | Structured file edit and patch application | Must | Workspace roots, locks and review policy enforced | An agent can propose and apply a validated patch only while holding required permissions and locks | diff/patch, tree-sitter | Reviewed |
| COD-002 | Coding Agent | Build, test and diagnostic loop | Must | Commands are bounded, audited and isolated | A task can run configured checks, persist outputs and feed diagnostics back into context | Terminal runtime | Reviewed |
| TRM-001 | Terminal | Sandboxed process and command execution | Must | Allowlist/policy, working-directory boundary and environment filtering | A command cannot access paths, secrets or capabilities outside its grant | Tokio process, sandbox TBD | Reviewed |
| TRM-002 | Terminal | PTY, streaming output, cancellation and resource limits | Must | Time, memory and output bounds | Long-running commands stream output, can be cancelled and always persist a terminal result | portable-pty or equivalent TBD | Reviewed |
| WF-001 | Workflow | YAML workflow definition | Must | Strict schema and versioning | Invalid workflows fail before execution with actionable errors | serde_yaml / JSON Schema | Reviewed |
| WF-002 | Workflow | DAG validation and cycle detection | Must | Deterministic validation | Cyclic graphs are rejected; valid graphs produce stable execution order | petgraph or custom DAG | Reviewed |
| WF-003 | Workflow | Parallel fan-out and fan-in | Must | Bounded concurrency | Independent worker tasks run concurrently and join at a defined barrier | Tokio tasks | Reviewed |
| WF-004 | Workflow | Retry, timeout and fallback | Must | Idempotency keys and retry limits | Retry policy does not repeat committed side effects | Custom DAG; Temporal inspiration | Reviewed |
| WF-005 | Workflow | Conditional branches and approvals | Must | Approval actor is authenticated and audited | Workflow pauses and resumes from a persisted approval decision | Custom DAG | Reviewed |
| WF-006 | Workflow | Rollback and compensation | Must | Explicit compensation contract | A failed workflow invokes declared compensation steps and records outcome | Saga pattern | Reviewed |
| WF-007 | Workflow | Durable resume | Must | Restart-safe execution | Runtime restart does not rerun completed nodes | Session persistence | Reviewed |
| EVT-001 | Event | Versioned event envelope | Must | Correlation, causation, session and actor metadata | All domain events validate against a published schema | Protobuf / serde | Reviewed |
| EVT-002 | Event | Local Pub/Sub | Must | Bounded queues and backpressure policy | Runtime components exchange typed events without direct ownership coupling | Tokio mpsc/broadcast | Reviewed |
| EVT-003 | Event | Durable production event transport | Must | Authentication, replay and consumer state | Durable consumers resume after disconnect and replay required events | NATS JetStream | Reviewed |
| EVT-004 | Event | Idempotent consumption and deduplication | Must | Duplicate delivery cannot corrupt state | Injected duplicate events produce one committed state transition | Event IDs + DB uniqueness | Reviewed |
| EVT-005 | Event | Dead-letter and retry handling | Must | Poison messages isolated | Failed events are observable and can be replayed after correction | NATS JetStream / custom local adapter | Reviewed |
| WSP-001 | Workspace | Workspace discovery and graph | Must | Root boundaries enforced | Runtime discovers directories and files without escaping configured roots | walkdir / ignore | Reviewed |
| WSP-002 | Workspace | File watching and incremental updates | Must | Symlink and path traversal safety | A file change updates only affected index entries and emits `FileChanged` | notify | Reviewed |
| WSP-003 | Workspace | Syntax and symbol graph | Must | Parser failures isolated | Supported languages expose file, symbol and reference metadata incrementally | tree-sitter, LSP | Reviewed |
| WSP-004 | Workspace | Text and symbol search | Must | Resource limits | Search returns ranked results under an agreed repository-size benchmark | ripgrep, Tantivy | Reviewed |
| WSP-005 | Workspace | Shared workspace snapshot | Should | Snapshot provenance | A task records the workspace revision used to build its context | Git + index metadata | Reviewed |
| LCK-001 | Collaboration | Hierarchical workspace/directory/file/symbol locking | Must | Conflict matrix is deterministic | Conflicting parent/child locks cannot be granted concurrently | Custom lock service | Reviewed |
| LCK-002 | Collaboration | Lease, heartbeat and timeout | Must | Dead owners cannot retain locks | Killing an agent releases or expires its locks within the configured bound | Runtime leases | Reviewed |
| LCK-003 | Collaboration | Wait queue and fairness | Should | Starvation prevention | Waiting requests are ordered by documented policy and expose queue state | Custom lock service | Reviewed |
| LCK-004 | Collaboration | Ownership transfer | Should | Transfer requires authorization | Lock transfer is atomic and appears in the audit timeline | Custom lock service | Reviewed |
| LCK-005 | Collaboration | Merge queue and conflict resolver contract | Must | Changes cannot bypass policy | Only eligible changes enter merge; conflicts create explicit resolution tasks | Git integration | Reviewed |
| GIT-001 | Git | Status, diff and revision identity | Must | Repository boundaries enforced | Session records repository identity, base revision and task diff | gitoxide, libgit2 | Reviewed |
| GIT-002 | Git | Branch or worktree isolation | Must | Agent changes isolated | Concurrent workers do not write to the same uncontrolled working tree | gitoxide/libgit2/Git CLI | Reviewed |
| GIT-003 | Git | Commit, merge and rollback | Must | Author and task provenance | Merge failure leaves repository recoverable and records the failed attempt | gitoxide, libgit2 | Reviewed |
| MEM-001 | Memory | Pluggable memory interface | Must | Data ownership and deletion | A memory backend can be replaced without changing session or agent contracts | Mem0, Graphiti, Zep, custom | Reviewed |
| MEM-002 | Memory | Memory classes and provenance | Must | Source, session, actor and timestamp required | Retrieved memory links to the source material that produced it | Custom metadata | Reviewed |
| MEM-003 | Memory | Retrieval, ranking and deduplication | Must | Query isolation and bounded cost | Benchmark reports relevance, latency and token contribution | Qdrant, BM25, graph retrieval | Reviewed |
| MEM-004 | Memory | Retention, deletion and privacy filtering | Must | Secrets excluded; deletion verifiable | Deleted or expired memory is absent from subsequent retrieval | Backend-specific | Reviewed |
| MEM-005 | Memory | Temporal and decision memory | Should | Conflicting facts retain validity history | Queries can distinguish current decisions from superseded decisions | Graphiti / temporal model | Reviewed |
| VEC-001 | Vector Store | Pluggable embedding and vector-store interface | Should | Provider and storage replacement without domain coupling | Qdrant or another backend can be replaced behind a stable memory contract | Qdrant, pgvector TBD | Reviewed |
| VEC-002 | Vector Store | Namespace, deletion, backup and rebuild | Should | Tenant/session isolation and verifiable deletion | A namespace can be deleted or rebuilt from source data without orphaned vectors | Qdrant, pgvector TBD | Reviewed |
| CTX-001 | Context | Incremental context assembly | Must | Full history is not broadcast by default | Context is built from task, dependencies, summaries, files, memory and locks | Custom engine | Reviewed |
| CTX-002 | Context | Token budget and compression | Must | Deterministic priority rules | Context never exceeds provider budget and records omitted sections | Tokenizers + summaries | Reviewed |
| CTX-003 | Context | Relevance explanation | Must | Auditability | Every included item records source and inclusion reason | Custom engine | Reviewed |
| CTX-004 | Context | Provider-aware formatting | Must | Preserve semantic requirements | Equivalent context is encoded according to provider capability constraints | Provider SDK | Reviewed |
| MCP-001 | MCP | MCP client and server support | Must | Capability negotiation and transport security | Runtime interoperates with official MCP conformance examples | Official Rust SDK (`rmcp`) | Reviewed |
| MCP-002 | MCP | Tool discovery and invocation | Must | Schema validation and least privilege | Discovered tools cannot execute without runtime permission evaluation | `rmcp` | Reviewed |
| MCP-003 | MCP | Cancellation, timeout and audit | Must | Every call bounded and attributable | Tool calls can be cancelled and produce a terminal audit record | `rmcp` + runtime policy | Reviewed |
| PLG-001 | Plugin | Plugin manifest, version and lifecycle | Must | Compatibility checks | Incompatible plugins fail before activation without crashing runtime | Custom Plugin SDK | Reviewed |
| PLG-002 | Plugin | Permission model and isolation | Must | Default deny | A malicious test plugin cannot read undeclared secrets or paths | Process/WASM isolation TBD | Reviewed |
| API-001 | API | gRPC/Protobuf client contract | Must | Authentication, TLS and versioning | CLI and IDE perform all durable operations through versioned services | tonic, prost | Reviewed |
| API-002 | API | Unary and bidirectional streaming | Must | Backpressure, reconnect and cancellation | Long-running execution streams progress and can reconnect/resume by ID | tonic | Reviewed |
| CLI-001 | Client | Headless CLI | Must | No direct DB/provider access | CLI creates, opens, resumes and inspects sessions through runtime APIs | clap | Reviewed |
| CLI-002 | Client | Interactive TUI | Should | Same API boundary as CLI | TUI displays session, task, agent, workflow, lock and event status | Ratatui | Reviewed |
| CLI-003 | Client | Stable machine-readable output | Must | Versioned schemas and no secret leakage | Automation can consume CLI output without parsing human text | clap + serde | Reviewed |
| IDE-001 | Client | VS Code extension | Must | Runtime remains alive when IDE closes | Extension reconnects to an existing session and restores views | VS Code API | Reviewed |
| SEC-001 | Security | Authentication and session authorization | Must | Explicit identity and access policy | Unauthorized clients cannot list or mutate sessions | TLS/OAuth/token TBD | Reviewed |
| SEC-002 | Security | Secret management and redaction | Must | Secrets never enter normal logs/events/memory | Automated tests inject secrets and verify redaction paths | Secret store TBD | Reviewed |
| SEC-003 | Security | Tool and plugin policy enforcement | Must | Default deny and auditable approval | High-risk actions require policy approval and produce audit entries | Custom policy engine | Reviewed |
| SEC-004 | Security | Dependency and artifact integrity | Must | Vulnerability scanning, SBOM and signed releases | Release is blocked by unaccepted critical findings and produces verifiable artifacts | cargo-audit/deny, signing TBD | Reviewed |
| OBS-001 | Observability | Structured logs, metrics and traces | Must | Correlation across session/task/agent/provider | One execution can be followed end-to-end by correlation ID | tracing, OpenTelemetry | Reviewed |
| OBS-002 | Observability | Cost, latency, failure and lock metrics | Must | Labels must avoid sensitive/high-cardinality data | Dashboards expose agreed service and product indicators | OpenTelemetry/Prometheus TBD | Reviewed |
| OBS-003 | Observability | Alerts and operational runbooks | Must | Alerts are actionable and owned | Each release-blocking alert links to a tested response or recovery procedure | Alerting stack TBD | Reviewed |
| DEP-001 | Deployment | Local single-node mode | Must | Minimal external dependencies | A documented command starts runtime with local storage and test/local provider | SQLite + local event adapter | Reviewed |
| DEP-002 | Deployment | Production service mode | Must | Durable storage, secure transport and recovery | Staging validates authenticated team use and production dependencies | PostgreSQL + NATS + optional Qdrant | Reviewed |
| DEP-003 | Deployment | Backup, restore, rolling upgrade and rollback | Must | Session durability across operations | A staged upgrade and rollback complete without unexplained session loss | Deployment stack TBD | Reviewed |
| TST-001 | Testing | Contract and conformance suites | Must | Repeatable in CI | Provider, event, RPC, plugin and MCP contracts run without external paid services | Mocks + official test suites | Reviewed |
| TST-002 | Testing | Recovery and chaos tests | Must | Forced failures included | Runtime, agent, transport and DB failures preserve documented invariants | Test harness | Reviewed |
| TST-003 | Testing | Performance benchmarks | Must | Reproducible datasets and hardware metadata | Baselines exist for session load, events, context and repository indexing | Criterion + scenario harness | Reviewed |
| TST-004 | Testing | Compatibility and migration tests | Must | Supported-version policy | Session, Protobuf, event and plugin upgrades are tested across supported versions | CI matrix | Reviewed |

## Product scope decisions for Phase 0

1. **Numeric SLOs:** Phase 0 must collect benchmark evidence; final SLA/SLO, RTO and RPO values are approved in the Phase 1 Production Specification.
2. **Initial workspace languages:** Rust, TypeScript/JavaScript and Python. They represent the runtime, VS Code/client ecosystem and major AI tooling ecosystem.
3. **Initial deployment scope:** local single-user mode plus authenticated, single-tenant team service mode. Multi-tenant SaaS isolation is outside the first production release.
4. **Mandatory approval classes:** destructive filesystem/Git actions, secret access, external side effects, privilege expansion and policy changes. The exact policy remains configurable and requires a Security RFC.
5. **Portable session data:** durable domain state, messages, summaries, decisions, task/workflow history, references and audit metadata. Credentials, transient caches and provider-internal opaque state are excluded.
6. **Minimum offline experience:** Runtime + CLI + SQLite + local event transport + Ollama-compatible provider. PostgreSQL, NATS and hosted providers are not required for offline local mode.
7. **Repository benchmark tiers:** small development corpus, medium project corpus and large stress corpus must be versioned and reproducible; numeric file/LOC thresholds are set in the workspace research report.

## Phase -1 review record

| Check | Result |
|---|---|
| Every `PROJECT.md` research and implementation domain represented | Passed after adding Coding Agent, Terminal and Vector Store capabilities |
| Every `Must` capability has an observable acceptance criterion | Passed for baseline; numeric targets deferred to Phase 0/1 evidence |
| Security requirements cover external boundaries | Passed for baseline |
| Recovery covers process, workflow, event and deployment failure | Passed for baseline |
| OSS entries remain candidates rather than approved dependencies | Passed |
| Product-level questions have explicit Phase 0 scope decisions | Passed |

**Gate result:** Phase -1 approved. Phase 0 Landscape Research may begin. Architecture, ADR, RFC and production implementation remain unapproved until their respective gates are completed.
