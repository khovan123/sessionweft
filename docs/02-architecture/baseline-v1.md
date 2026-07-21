# SessionWeft Architecture Baseline v1

Status: **Approved for vertical-slice implementation**  
Date: 2026-07-22

## 1. System context

```text
Human / Automation
        |
        v
 CLI / IDE / SDK  ---- versioned API ---->  SessionWeft Runtime
                                                |
                +-------------------------------+------------------+
                |               |               |                  |
                v               v               v                  v
          Session Store    Event Transport  Provider Adapters  Tool/MCP Gate
                |               |               |                  |
          SQLite/Postgres   Local/JetStream  Ollama/Cloud APIs  Sandboxed tools
                |
         Rebuildable projections
       workspace / memory / vectors
```

## 2. Runtime domains

| Domain | Owns | Must not own |
|---|---|---|
| Session Engine | Session aggregate, versions, timeline references | Provider-native conversation identity |
| Runtime Service | Use-case orchestration and policy checks | Client UI state |
| Storage | Atomic aggregate and outbox persistence | Business decisions |
| Event Engine | Delivery and retry | Authoritative Session state |
| Provider Layer | API translation and normalized output | Durable conversation history |
| Workflow Engine | Persisted node execution state | Hidden worker-only progress |
| Collaboration | Locks, leases, fencing, merge eligibility | Untracked filesystem ownership |
| Workspace | Rebuildable file/symbol projections | Unversioned source of truth |
| Memory/Context | Typed records and context selection | Secret storage or implicit authority |
| Tool/MCP Gate | Discovery, policy, execution audit | Unreviewed direct model execution |
| Clients | Presentation and commands | Durable Session state |

## 3. Session aggregate

The first aggregate contains:

- `session_id` — Runtime-generated stable UUID;
- `version` — monotonically increasing optimistic-concurrency token;
- lifecycle status;
- title and timestamps;
- message history for the first vertical slice;
- selected provider and model;
- references to workflow, task, workspace, locks, memory and Git projections;
- audit metadata.

Later high-volume data may move to referenced tables, but aggregate invariants remain controlled by the Session Engine.

## 4. Command transaction

```text
Client command
  -> authenticate and authorize
  -> load Session version N
  -> validate command
  -> produce Session version N+1 and domain events
  -> transaction:
       compare-and-swap Session where version = N
       insert outbox events
     commit
  -> return committed state
  -> asynchronous outbox publisher delivers events
```

A failed compare-and-swap returns a typed conflict. The Runtime does not silently overwrite concurrent changes.

## 5. Event topology

### Local mode

- Bounded Tokio channels provide in-process notifications.
- The SQLite outbox remains the durable publication record.
- Restart replays unpublished outbox rows.

### Service mode

- PostgreSQL stores aggregate and outbox atomically.
- A publisher sends events to NATS JetStream.
- Pull consumers acknowledge after idempotent processing.
- Duplicate delivery is expected and tested.

## 6. Provider topology

The Runtime builds provider requests from Session-owned context. Adapters translate the common request to each provider API. Responses are normalized into typed events. Provider IDs are metadata only.

The first adapters are:

- deterministic Echo adapter for tests;
- Ollama-compatible adapter for offline/local use.

Cloud adapters are added only through the same conformance suite.

## 7. Security boundaries

1. Client boundary — authentication, authorization, request limits.
2. Provider boundary — credentials, data minimization, timeout and cost limits.
3. Tool/MCP boundary — schema validation, policy decision, approval, sandbox and audit.
4. Workspace boundary — canonical roots, path traversal and symlink controls.
5. Plugin boundary — process identity, declared capabilities and resource limits.
6. Telemetry boundary — redaction and bounded cardinality.

## 8. Deployment modes

### Local

- loopback bind by default;
- SQLite WAL on local disk;
- local event adapter;
- optional Ollama-compatible provider;
- no externally reachable endpoint unless explicitly configured.

### Team service

- authenticated API;
- PostgreSQL;
- NATS JetStream;
- managed secrets;
- TLS at the Runtime or trusted ingress;
- backup, restore and rolling-upgrade procedures.

## 9. Failure invariants

- Process crash cannot create a committed Session mutation without an outbox record.
- Event redelivery cannot apply a durable transition twice.
- Provider failure cannot erase committed user input.
- Client disconnect does not stop Runtime-owned work unless cancellation is explicitly requested.
- Agent lease loss prevents future protected commits through fencing-token validation.
- Projection loss is recoverable from authoritative Session, Git and source records.

## 10. Vertical-slice boundary

The first code increment implements Session, SQLite WAL, outbox, local events, provider contract, Echo/Ollama adapters, an authenticated API and CLI. Workflow, locking, workspace indexing, memory, MCP and IDE remain behind stable extension traits until their implementation PRs.
