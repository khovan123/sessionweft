# SessionWeft Release-Candidate Threat Model

Status: **Architecture and security baseline approved for RC**.

## Protected assets

- Session state, summaries, decisions, Memory and Workflow state.
- Repository contents, Git history, worktrees and merge refs.
- Provider, MCP and deployment credentials.
- Lock leases, fencing tokens, task claims and approval grants.
- PostgreSQL, JetStream, Outbox and Inbox records.
- Audit events, client cursors and release artifacts.

## Trust boundaries

1. CLI, TUI and VS Code are untrusted clients of the authenticated Runtime API.
2. Providers and MCP servers are external systems and never own Session state.
3. Plugin processes are untrusted child processes with explicit capability policy.
4. PostgreSQL is the service-mode source of truth; JetStream is transport, not state authority.
5. Workspace files and Git repositories may contain adversarial names, symlinks and content.
6. CI dependencies, container images and release tooling are supply-chain boundaries.

## Threats and required controls

| Threat | Example | Required control | Evidence |
|---|---|---|---|
| Spoofing | forged client or Runtime identity | bearer authentication, stable unique Runtime instance IDs, constant-time token comparison | daemon auth tests and service-mode configuration |
| Tampering | stale Agent writes after lease expiry | hierarchical locks, monotonic fencing tokens, validation immediately before write/stage/commit/merge | scheduler and Git fence integration tests |
| Repudiation | privileged mutation without trace | correlation IDs, actor identity, transactional Outbox and durable client event journal | mutation audit tests |
| Information disclosure | plugin reads secrets or parent directories | cleared environment, canonical workspace root, bubblewrap filesystem/network policy, secret scan | malicious-plugin and secret-leakage tests |
| Denial of service | plugin hangs or floods output | initialization and operation timeout, cancellation, bounded output, rate limits and bounded queues | malicious MCP fixtures and SLO alerts |
| Elevation of privilege | discovered MCP tool bypasses policy | default-deny Tool policy, named permission, risk classification and one-time durable approval | approval consumption tests |
| Duplicate side effect | redelivery or worker restart repeats tool call | execution ledger, idempotency keys, Inbox uniqueness and `Uncertain` state | execution and JetStream redelivery tests |
| Lost update | two Runtime instances write same version | expected-version compare-and-swap and transactional state + Outbox | PostgreSQL conflict tests |
| Split brain | two workers own task/lock | expiring task claims, workspace serialization, `FOR UPDATE SKIP LOCKED` and fencing | two-Runtime tests |
| Repository overwrite | target ref changed during merge | `git update-ref` compare-and-swap and no force merge | real-Git merge tests |
| Path traversal | crafted file path or symlink escapes workspace | canonical root validation, no symlink following, normalized relative paths | Workspace and process-runner tests |
| Supply-chain compromise | malicious crate/action/artifact | locked dependencies, advisory scan, SBOM, pinned release workflow and provenance attestation | production-hardening and release workflows |
| Backup compromise | unverified or stale restore | isolated restore drill, count verification, RPO metadata and restricted backup access | backup/restore workflow |

## Secret-handling rules

- Credentials come from the deployment secret manager or GitHub encrypted secrets.
- Secrets must not be stored in Session, Memory, Workflow payloads, logs, metric labels, issue bodies or release artifacts.
- Child process environments are cleared and rebuilt from an allowlist.
- The release gate scans all tracked text files for high-confidence credential formats.
- Test credentials are limited to isolated local containers and must use non-production values.

## Residual risks accepted for RC

- Single-region PostgreSQL/NATS deployment remains an operator responsibility; SessionWeft provides recovery semantics but not a managed control plane.
- Bubblewrap is the Linux production baseline. Other operating systems require an equivalent sandbox before external plugins are enabled.
- LSP servers are not part of the RC workspace-intelligence dependency chain.
- Keyless provenance attestation depends on GitHub Actions OIDC availability.
- Formal third-party penetration testing is required before a public multi-tenant GA offering.

## Release blockers

Any unresolved finding with Critical or High severity blocks RC. The following findings block release regardless of assigned severity:

- stale fencing token accepted;
- duplicate external side effect after recovery;
- secret or private key in source/artifact/log output;
- unauthenticated non-loopback mutation;
- restore drill unable to recover committed state;
- release artifact without checksum, SBOM and provenance attestation.
