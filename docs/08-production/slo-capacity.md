# SessionWeft Release-Candidate SLO and Capacity Targets

Status: **Approved RC baseline**. These targets apply to the first authenticated single-tenant service-mode release. Provider and external tool execution time is reported separately from Runtime control-plane latency.

## Availability and latency

| Signal | RC objective | Measurement window | Release-blocking threshold |
|---|---:|---:|---:|
| Runtime control-plane availability | 99.90% | rolling 30 days | below 99.80% |
| Authenticated read API latency | p95 ≤ 250 ms, p99 ≤ 1 s | 5 minutes | p99 > 2 s for 10 minutes |
| Authenticated mutation API latency | p95 ≤ 500 ms, p99 ≤ 2 s | 5 minutes | p99 > 4 s for 10 minutes |
| PostgreSQL task-claim failover | ≤ 10 s after lease expiry | each failover | > 15 s |
| Stale fencing-token rejection | ≤ 1 s after lease expiry | each validation | any stale mutation accepted |
| Outbox-to-JetStream publish latency | p95 ≤ 2 s, p99 ≤ 10 s | 5 minutes | p99 > 20 s for 10 minutes |
| JetStream consumer redelivery | ≤ 15 s | each interrupted delivery | > 30 s |
| Client event resume | ≤ 5 s from last durable cursor | reconnect | cursor gap or duplicate side effect |

Provider and MCP/tool calls have a separate budget: Runtime must begin the external call or return a bounded authorization/transport error within 2 seconds. Provider-specific completion latency is not part of the control-plane SLO.

## Recovery objectives

| Failure domain | RTO | RPO | Required evidence |
|---|---:|---:|---|
| Single Runtime process | 5 minutes | 0 for committed PostgreSQL state | kill/restart and claim takeover test |
| NATS/JetStream process | 15 minutes | 0 after acknowledged publish; at-least-once before ACK | pause/unpause and redelivery test |
| PostgreSQL primary or deployment database | 30 minutes | ≤ 5 minutes | verified backup/restore drill |
| Complete service deployment rollback | 30 minutes | ≤ 5 minutes | compatibility and rollback runbook |
| Local SQLite mode | 15 minutes | last successful filesystem backup | local backup procedure; not HA |

## Verified RC capacity baseline

The RC gate verifies the following bounded baseline. Higher numbers require a new evidence record rather than an undocumented configuration increase.

| Resource | Verified baseline |
|---|---:|
| Concurrent Runtime instances sharing service-mode state | 2 mandatory; design limit 10 |
| Concurrent contenders for one task or conflicting lock | 32 |
| Active Sessions per deployment | 1,000 |
| Active Agents per deployment | 2,000 |
| Pending Workflow nodes | 10,000 |
| Durable client events retained per deployment | 1,000,000 before archival |
| Indexed workspace files | 1,000 in mandatory RC CI; 10,000 scheduled capacity target |
| Context candidates assembled per request | 5,000 |
| MCP tools per server | bounded by SDK adapter limit; no unbounded discovery |
| Provider/Tool output | bounded by adapter-specific byte limit |

## Error-budget policy

A 99.90% monthly SLO allows approximately 43 minutes of control-plane unavailability in a 30-day month. Release work stops when either condition is true:

1. more than 50% of the monthly error budget is consumed in seven days;
2. any stale lock/fence, duplicate side effect, secret exposure, unrecoverable migration or data-loss incident occurs.

Security and consistency failures have a zero error budget. They block RC promotion regardless of aggregate availability.

## Measurement rules

- Health probes do not count as user traffic.
- Requests rejected before authentication are counted separately.
- Latency starts when Runtime accepts the request and ends when the durable mutation or response is available.
- External Provider/MCP latency is split from Runtime authorization and persistence latency.
- Retries are measured individually and by end-to-end operation correlation ID.
- Metrics labels must remain bounded; Session, Agent, Workflow, file paths and correlation IDs must not become metric labels.
