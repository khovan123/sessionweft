# Release Candidate Gate

## Decision levels

SessionWeft has three machine-readable gate levels:

- **Preflight** — code quality and required technical evidence exist.
- **Release Candidate** — all technical gates pass, zero Critical/High findings remain and architecture/security/operations RC reviews are recorded.
- **General Availability** — every RC requirement plus human architecture, security and operations approval.

Automated evidence can authorize an RC build. It cannot authorize GA.

## Numeric targets for `0.1.0-rc.1`

| Target | Value |
|---|---:|
| Monthly service availability | 99.9% |
| API read p95 | ≤250 ms |
| API mutation p95 | ≤500 ms |
| Outbox publish to durable consumer p95 | ≤2 s |
| Ready task to claim p95 | ≤2 s |
| Service RTO | ≤30 min |
| Service RPO | ≤5 min |
| Local committed-state RPO | 0 s |
| Concurrent Sessions | 100 |
| Active Agents | 50 |
| Queued tasks | 10,000 |
| Indexed source files/workspace | 10,000 release target; 50,000 configured hard limit |
| Durable event backlog | 1,000,000 |

## Required technical evidence

- locked Rust build, format, Clippy and workspace tests;
- two-Runtime PostgreSQL/JetStream ownership and idempotence tests;
- restart/network-partition/provider-outage chaos drill;
- client cursor/PTY reconnect tests;
- 10,000-file workspace capacity profile;
- PostgreSQL backup/restore and migration compatibility drills;
- RustSec, source/license policy and tracked-secret scan;
- malicious MCP plugin and privilege-boundary tests;
- CycloneDX SBOM, checksums and provenance-attested artifacts;
- dashboard, alerts and operational runbooks.

## Commands

```bash
cargo run -p sessionweft-release-gate --locked -- \
  --policy release/release-policy.json \
  --evidence release/evidence/rc-0.1.0.json \
  --level preflight

cargo run -p sessionweft-release-gate --locked -- \
  --policy release/release-policy.json \
  --evidence release/evidence/rc-0.1.0.json \
  --level rc
```

A GA command is expected to fail until the automated sign-off entries are replaced or supplemented with human `approved_for_ga` records:

```bash
cargo run -p sessionweft-release-gate --locked -- \
  --policy release/release-policy.json \
  --evidence release/evidence/rc-0.1.0.json \
  --level ga
```

## Release-blocking findings

- any open Critical or High security finding;
- missing/waived technical evidence at RC;
- failed restore or migration drill;
- duplicate side effect, task owner or exclusive lock owner;
- stale fencing token accepted for mutation;
- plugin/workspace escape or secret leakage;
- unsigned/unattested release artifact;
- missing architecture, security or operations RC review.
