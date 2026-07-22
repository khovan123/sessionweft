# Production Readiness Targets

Status: Release Candidate baseline for `0.1.0-rc.1`.

## Service objectives

| Signal | Target | Measurement window |
|---|---:|---|
| Runtime API availability | 99.9% | rolling 30 days |
| Read API p95 latency | <= 250 ms | 5-minute windows |
| Mutation API p95 latency | <= 500 ms | 5-minute windows |
| Event delivery p95 | <= 2 seconds | publish to successful handler |
| Scheduler claim p95 | <= 2 seconds | ready node to durable claim |
| Critical/High findings | 0 open | release gate |

## Recovery objectives

| Mode | RTO | RPO |
|---|---:|---:|
| PostgreSQL/JetStream service mode | 30 minutes | 5 minutes |
| SQLite local mode committed state | 10 minutes | 0 seconds after successful commit |

A recovery drill fails when the restored database cannot reproduce Session counts, durable locks, pending Outbox state and Inbox idempotency evidence.

## Capacity baseline

The first RC is validated for at least:

- 100 concurrent Sessions;
- 50 active Agents;
- 10,000 queued tasks;
- 10,000 indexed files per workspace;
- 1,000,000 pending/replayable events.

These are release qualification targets, not contractual customer limits. A later release must update `release/release-policy.json` before claiming higher capacity.

## Error budget

A 99.9% monthly availability target permits approximately 43 minutes of unavailable time in a 30-day month. The alert policy pages when the projected burn rate exceeds the monthly budget and blocks release when a critical path SLO has no measurement source.
