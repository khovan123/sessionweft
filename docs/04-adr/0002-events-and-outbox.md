# ADR-0002: Transactional Outbox and Event Transport Boundary

- Status: Accepted
- Date: 2026-07-22
- Issues: #3, #10

## Context

SessionWeft needs low-latency local collaboration and durable production delivery. Core NATS, in-process channels and JetStream have different delivery guarantees. No transport can replace the Session database as the authority.

## Decision

1. Define a versioned `EventEnvelope` with event ID, type, schema version, session ID, actor ID, correlation ID, causation ID and timestamp.
2. Insert outbox rows in the same database transaction as state changes.
3. Define an `EventTransport` interface.
4. Use bounded Tokio channels for in-process delivery.
5. Use NATS JetStream pull consumers for production durable delivery.
6. Assume at-least-once delivery for all durable consumers.
7. Require stable event IDs and idempotent handlers.
8. Acknowledge durable messages only after the handler commits its result.

## Consequences

- A crash between publish and outbox acknowledgement may create duplicate delivery.
- Consumer deduplication is a domain requirement, not an optional transport feature.
- Local notifications can be dropped under configured backpressure policy, while the durable outbox remains recoverable.
- Event schemas require compatibility tests and explicit versioning.

## Alternatives

- NATS as the source of truth: rejected.
- Unbounded in-memory queues: rejected.
- Claiming application-level exactly-once behavior from JetStream alone: rejected.
- Publishing before database commit: rejected.

## Operations

Outbox age, publish failures, retry count, dead-letter count and consumer lag are mandatory metrics. Operators must be able to replay a bounded event range by ID and time without bypassing idempotency checks.
