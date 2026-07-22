# SessionWeft SaaS HTTP API v2

Status: SessionWeft 0.2.0 implementation contract.

## Authentication

Tenant routes require `Authorization: Bearer <tenant-token>`. Tenant tokens are random, returned once, persisted only as SHA-256 digests and resolved to a tenant membership before a Runtime schema is selected.

The tenant identifier in the URL is never trusted by itself. A valid token for another tenant receives `404 not_found` rather than evidence that the requested tenant or resource exists.

Tenant bootstrap is a separate administrative operation protected by `X-SessionWeft-Bootstrap-Token`. The bootstrap token is configured out of band, hashed before comparison and compared in constant time.

## Routes

```text
GET  /health
POST /v2/bootstrap/tenants
POST /v2/tenants/{tenant_id}/tokens
PUT  /v2/tenants/{tenant_id}/quotas
POST /v2/tenants/{tenant_id}/sessions
GET  /v2/tenants/{tenant_id}/sessions/{session_id}
POST /v2/tenants/{tenant_id}/sessions/{session_id}/agents
POST /v2/tenants/{tenant_id}/sessions/{session_id}/workflows
GET  /v2/tenants/{tenant_id}/sessions/{session_id}/locks
POST /v2/tenants/{tenant_id}/sessions/{session_id}/locks
PUT  /v2/tenants/{tenant_id}/billing/plans
POST /v2/tenants/{tenant_id}/billing/subscriptions
GET  /v2/tenants/{tenant_id}/billing/entitlements/{name}
POST /v2/tenants/{tenant_id}/billing/usage
POST /v2/billing/stripe/webhook
```

## Runtime isolation

Each tenant Runtime uses a dedicated PostgreSQL schema named from the canonical tenant UUID. Every pool connection sets a fixed search path to that schema before queries are accepted. Session, Agent, Workflow, Lock and Memory repositories therefore cannot resolve rows from another tenant schema.

The SaaS authority and billing tables additionally use forced PostgreSQL row-level security with transaction-local tenant context. The database role used by the Runtime must be non-superuser and must not have `BYPASSRLS`.

## Quotas and idempotency

Creating a Session requires a Session quota reservation and a client idempotency key. Quota reservations are replay-safe and commit usage plus audit/outbox state atomically. Other resource dimensions are reserved by their owning application service before external execution.

Billing usage uses its own idempotency key and crash-safe ledger. A `Reported` record is not sent twice; an `Uncertain` record requires provider reconciliation.

## Stripe webhook

The Stripe webhook route receives the raw request body. It verifies the `Stripe-Signature` HMAC and timestamp before parsing JSON, obtains tenant identity only from verified object metadata and deduplicates provider event IDs before changing subscription state.

## Error envelope

```json
{
  "error": {
    "code": "stable_machine_code",
    "message": "safe human-readable summary"
  }
}
```

Authentication failures return `401`, role failures `403`, cross-tenant or absent resources `404`, invalid input `400`, quota exhaustion `429`, missing subscription `402` and unavailable payment-provider operations `503`. Internal database details and secrets are never returned.
