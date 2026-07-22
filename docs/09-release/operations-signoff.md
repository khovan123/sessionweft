# Operations Sign-off — `0.1.0-rc.1`

Decision: **Approved for Release Candidate; not approved for General Availability**.

Reviewer: `sessionweft-automation`  
Review type: automated operations readiness review  
Date: 2026-07-22

## Evidence reviewed

- numeric SLO, RTO, RPO and capacity policy;
- service-mode health and ownership tests;
- PostgreSQL/NATS restart and partition drill;
- PostgreSQL isolated restore drill;
- additive migration compatibility drill;
- Runtime/client reconnect behavior;
- Prometheus textfile exporter, alert rules and dashboard;
- incident, recovery, upgrade and rollback runbooks;
- release packaging, checksums and provenance gate.

## Findings

- No operations evidence blocks an RC when the hardening workflow passes.
- Recovery actions preserve task claims, fencing tokens, Outbox/Inbox identities and event cursors.
- GA remains blocked until operators execute the drills in the target production environment and record measured RTO/RPO, alert routing and backup retention evidence.
