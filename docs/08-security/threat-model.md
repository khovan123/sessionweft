# SessionWeft Production Threat Model

Status: Release Candidate baseline for `0.1.0-rc.1`.

## Security objectives

SessionWeft must preserve:

1. **Session integrity** — providers, plugins and clients cannot own or replace durable Runtime state.
2. **Workspace integrity** — only the current lock owner with the current fencing token may mutate protected resources.
3. **Side-effect idempotence** — restart or redelivery cannot blindly repeat external actions.
4. **Credential confidentiality** — secrets are never written to events, logs, plugin environments, client storage other than platform secret storage, or release artifacts.
5. **Tenant boundary** — authenticated team-service requests cannot read or mutate another Session/workspace scope.
6. **Recovery evidence** — Outbox, Inbox, approval, claim and merge records remain auditable after failure.

## Trust boundaries

| Boundary | Trusted component | Untrusted input |
|---|---|---|
| Client → Runtime API | Runtime authentication, Control Plane | CLI/TUI/VS Code payloads, cursors, PTY input |
| Runtime → Provider | Provider adapter and execution ledger | Provider output, latency, usage metadata |
| Runtime → MCP plugin | Policy gateway, one-time approval, sandbox | Tool schema, output, process behavior |
| Runtime → Workspace/Git | Canonical root, locks, fencing, worktrees | Repository content, symlinks, Git conflicts |
| Runtime → PostgreSQL | Repository transactions and CAS | Network failures, stale writers, restored data |
| Runtime → JetStream | Event envelope, Inbox idempotence | Duplicate delivery, poison events, replay order |
| Release pipeline → users | Locked build, SBOM, checksums, provenance | Dependency registry, runner environment |

## Principal threats and controls

### Spoofing and authentication bypass

- Non-loopback Runtime binds require a bearer token.
- Token comparison is constant time.
- VS Code stores tokens in SecretStorage and never workspace settings.
- Every mutation records an actor and correlation identifier.

Release blocker: any unauthenticated mutation or cross-Session access.

### Tampering and stale ownership

- Session, Workflow and Agent writes use expected-version compare-and-swap.
- Hierarchical leases use monotonic fencing tokens.
- Git stage, commit, rebase and merge revalidate the live fence.
- Target refs update with expected-old-object compare-and-swap.
- PostgreSQL task claims and lock acquisition serialize competing Runtime instances.

Release blocker: stale writer can commit after lease loss or two Runtime instances own one exclusive resource.

### Duplicate side effects

- State mutation and Outbox append commit in one transaction.
- Inbox uniqueness is `(consumer_name, event_id)`.
- Scheduler claims and execution records carry deterministic idempotency keys.
- Execution transitions to `Running` before the external call; stale `Running` becomes `Uncertain` and is not blindly retried.
- MCP approvals are one-time compare-and-set records consumed before invocation.

Release blocker: duplicate event delivery invokes a handler twice or a stale execution automatically repeats an uncertain external call.

### Malicious providers and plugins

- Provider conversation identifiers are metadata only; Runtime Session IDs remain authoritative.
- MCP tool names are namespaced and duplicate/schema-spoofed tools are rejected.
- Initialization and operations are bounded by timeout and cancellation.
- Stdio plugins run through bubblewrap with cleared environment, explicit filesystem binds and network denied by default.
- Output count and size are bounded.

Release blocker: plugin accesses undeclared secrets/files/network, survives cancellation, or bypasses approval policy.

### Workspace escape and repository attacks

- Workspace roots and requested paths are canonicalized.
- Symlinks are not followed by workspace indexing.
- Agent work occurs in isolated Git worktrees.
- Dirty indexes are rejected before automated stage/commit.
- Conflicts become durable tasks; no force merge is permitted.

Release blocker: path traversal, symlink escape, unreviewed merge or destructive force update.

### Data loss and corruption

- Local SQLite uses WAL and atomic state+Outbox transactions.
- Service mode uses PostgreSQL transactions and durable JetStream storage.
- Backup artifacts are verified by restore into an isolated database.
- Persisted workspace snapshots are schema-versioned and revision-verified.
- Migrations are additive by default and tested twice for idempotence.

Release blocker: verified backup cannot restore, migration deletes legacy data, or committed state disappears after a restart drill.

### Supply-chain compromise

- Cargo.lock is mandatory.
- RustSec and cargo-deny checks are release blocking.
- Unknown Git/registry sources are denied.
- Tracked secret patterns are rejected.
- CycloneDX SBOM, archive checksums and GitHub provenance attestations are attached to release artifacts.

Release blocker: unresolved Critical/High advisory, unknown source, missing SBOM/checksum/provenance or committed private key/token.

## Abuse cases tested

- Provider unavailable after the user input is committed.
- Runtime/consumer restart and duplicate JetStream delivery.
- Competing task claims and exclusive lock acquisition.
- MCP hang during initialization, tool collision, schema spoofing, output flood and secret probe.
- Workspace parent traversal and symlink boundary.
- Lease revoked between stage and commit.
- Git rebase conflict, interrupted merge and target ref movement.
- Client disconnect while Runtime-owned PTY continues.

## Residual risk

- Bubblewrap is Linux-specific. Other operating systems require equivalent process sandbox adapters before production plugin execution.
- The first RC uses conservative lexical import resolution before optional LSP reference indexing.
- Automated RC sign-offs do not authorize GA. Human architecture, security and operations sign-offs remain mandatory.

## Finding policy

- Critical and High findings: zero open at RC and GA.
- Medium findings: require owner, remediation date and documented non-exploitability for the release scope.
- Low findings: tracked in backlog and reviewed at the next release gate.
