# Production Readiness Checklist

Status: **Initial Phase -1 baseline**

Numeric targets remain TBD until Phase 1. A subsystem cannot be marked production-ready by feature completeness alone.

## Architecture and ownership

- [ ] Runtime ownership of durable state is documented.
- [ ] Client, provider, plugin and agent boundaries are explicit.
- [ ] Session invariants are covered by tests.
- [ ] External dependencies are behind replaceable interfaces.
- [ ] Every material dependency has an approved ADR.

## Reliability and recovery

- [ ] Graceful shutdown is implemented and tested.
- [ ] Forced process termination recovery is tested.
- [ ] Retries are bounded and classified by error type.
- [ ] Side-effecting operations use idempotency or compensation.
- [ ] Stale agent and lock ownership expires safely.
- [ ] Workflow resume does not rerun completed work.
- [ ] Backup and restore are tested from actual artifacts.
- [ ] Recovery objectives are defined in the Production Specification.

## Data integrity

- [ ] Session updates use transaction and version boundaries.
- [ ] Duplicate events cannot produce duplicate committed transitions.
- [ ] Schema migrations have forward and rollback procedures.
- [ ] Audit and timeline records retain causation and correlation.
- [ ] Export and deletion behavior is documented.

## Security

- [ ] Threat model is current.
- [ ] Client authentication is enabled outside explicitly local mode.
- [ ] Session authorization is enforced server-side.
- [ ] Secrets are stored outside source and normal session data.
- [ ] Logs, events and memory redact sensitive values.
- [ ] Tool and plugin permissions are default-deny.
- [ ] High-risk actions have approval policy.
- [ ] Dependency and artifact scanning are part of release gates.
- [ ] Security incident and credential rotation runbooks exist.

## Observability

- [ ] Structured logs use stable field names.
- [ ] Session, task, agent and request correlation works end-to-end.
- [ ] Metrics cover latency, errors, saturation and business/runtime state.
- [ ] Provider usage and cost are attributable.
- [ ] Event lag and redelivery are visible.
- [ ] Lock contention and stale leases are visible.
- [ ] Workflow failures and retries are visible.
- [ ] Alerts link to actionable runbooks.

## Performance and capacity

- [ ] Session load/save benchmarks exist.
- [ ] Event throughput and replay benchmarks exist.
- [ ] Repository indexing benchmark uses a documented corpus.
- [ ] Context assembly reports latency and token size.
- [ ] Provider concurrency and rate-limit behavior are tested.
- [ ] Memory retrieval benchmark reports quality and latency.
- [ ] Capacity limits and backpressure behavior are documented.

## Compatibility

- [ ] Protobuf and event schema compatibility policy exists.
- [ ] Provider adapters run a shared conformance suite.
- [ ] Plugin and MCP compatibility is checked before activation.
- [ ] Rolling upgrades are tested against supported client versions.
- [ ] Session migrations preserve resumability.

## Operations and release

- [ ] Local development environment is reproducible.
- [ ] Staging represents production dependencies and topology.
- [ ] Deployment, upgrade and rollback procedures are tested.
- [ ] Release artifacts are versioned and signed.
- [ ] Software Bill of Materials is produced.
- [ ] Changelog and migration guidance are published.
- [ ] Critical and high-severity defects are resolved or explicitly accepted.
- [ ] Architecture, security and operations sign-off are recorded.
