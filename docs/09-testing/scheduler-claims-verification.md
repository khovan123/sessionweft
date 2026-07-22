# Scheduler Claims Verification

Status: verification in progress for issue #30 and PR #46.

The final gate requires:

- dependency lockfile validation;
- rustfmt verification;
- Clippy with warnings denied;
- all workspace tests;
- read-only GitHub Actions permissions;
- no temporary source-mutating workflow remaining on `main`.

The verified scheduler claim boundary must preserve atomic Workflow, Agent, Claim and Outbox updates and deterministic idempotency keys.
