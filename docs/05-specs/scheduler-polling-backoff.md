# Scheduler Polling and Backoff Loop

Status: implementation and verification slice for issue #30.

Each bounded scheduler tick performs these stages in order:

1. recover stale active claims;
2. hand over released retryable claims;
3. scan persisted Scheduler Plans for ready Workflow nodes;
4. find fresh running Agents without a current task;
5. claim matching work through the existing atomic claim transaction.

The daemon starts one polling task with the Runtime cancellation token and joins it during graceful shutdown.

Configuration:

- `SESSIONWEFT_SCHEDULER_BATCH_LIMIT`, default `100`;
- `SESSIONWEFT_SCHEDULER_MIN_BACKOFF_MS`, default `100`;
- `SESSIONWEFT_SCHEDULER_MAX_BACKOFF_MS`, default `5000`.

A tick that commits any transition resets delay to the minimum. An idle or failed tick doubles the delay up to the configured maximum. Atomic repository operations remain responsible for concurrency and idempotency.
