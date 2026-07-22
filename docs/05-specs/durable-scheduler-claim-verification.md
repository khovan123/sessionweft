# Durable Scheduler Claim Verification

Status: implementation verification in progress.

The first scheduler claim slice must prove:

- Scheduler Plans only reference task nodes in the persisted Workflow;
- role and capability requirements are evaluated before claiming;
- one SQLite transaction updates Workflow, Agent, Task Claim and outbox;
- one Workflow node has at most one active claim;
- every claim exposes a deterministic idempotency key;
- completion and matching terminal replay are idempotent;
- capability mismatch leaves the ready node unclaimed;
- final CI is read-only and passes lockfile, format, Clippy and all workspace tests.

Stale-Agent handover and the active scheduler loop remain follow-up work under issue #30.
