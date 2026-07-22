# Multi-tenant SaaS and Billing Specification

Status: implementation baseline for SessionWeft 0.2.0.

## Scope

This specification expands the 0.1.0 single-tenant service mode with tenant identity, membership, resource ownership, quotas, subscriptions, entitlements and usage billing.

## Tenant boundary

Every request is resolved to a `TenantContext` containing a tenant ID, principal ID, roles and correlation ID. Client-supplied tenant IDs are never accepted without a membership lookup. Tenant resource lookups return not-found rather than revealing another tenant's existence.

PostgreSQL is defense in depth:

- each tenant table has a `tenant_id` column, or uses its tenant primary key;
- row-level security is enabled and forced;
- the tenant ID is set with transaction-local `set_config('sessionweft.tenant_id', ..., true)`;
- no tenant query is executed outside the transaction that established the context;
- missing context is default-deny;
- quota reservation, resource ownership and audit/outbox writes are atomic.

PostgreSQL's table owner normally bypasses row security, so SessionWeft uses `FORCE ROW LEVEL SECURITY` for Runtime-owned tenant tables. Policy expressions use current-row values and avoid cross-table subqueries.

## Roles

- `owner`: tenant lifecycle, members, billing and Runtime mutation;
- `admin`: members, billing and Runtime mutation;
- `billing`: subscriptions and invoices only;
- `member`: Runtime mutation only;
- `viewer`: read-only access.

## Quotas

Supported dimensions include Sessions, active Agents, queued tasks, indexed files, event backlog, provider tokens, tool invocations and storage bytes. Reservations are:

- hard-limit and fail-closed;
- serialized with row locks;
- append-only by idempotency key;
- returned unchanged on replay;
- audited in the SaaS Outbox transaction.

## Billing ownership

SessionWeft owns plans, subscriptions, entitlement decisions and the usage ledger. A payment provider owns payment processing only. Loss of the provider does not erase local entitlement or usage history.

Usage follows a crash-safe state machine:

```text
Prepared -> Reporting -> Reported
                    -> Failed
                    -> Uncertain
```

The same idempotency key is used for retries. A record already marked `Reported` is never sent again. An uncertain result requires reconciliation using the provider event identifier or the same idempotent request.

## Stripe adapter

The reference adapter:

- maps SessionWeft plans to configured Stripe Price IDs;
- creates customers and subscriptions with separate idempotency keys;
- reports usage using Stripe Billing meter events;
- records tenant identity in object metadata;
- verifies `Stripe-Signature` over the raw body with timestamp tolerance;
- deduplicates webhook event IDs before applying normalized subscription state;
- never logs API keys, webhook secrets or unredacted payment payloads.

## Release blockers

- a cross-tenant read or mutation;
- a table without forced RLS in the tenant schema;
- resource ownership that can be rebound across tenants;
- quota double-counting on idempotent replay;
- duplicate provider usage caused by retry;
- webhook processing without signature verification and event deduplication;
- entitlement decisions made solely from unverified provider state.

## Primary references

- PostgreSQL 18 row security policies and `FORCE ROW LEVEL SECURITY`.
- Stripe Billing usage meters, idempotent requests and subscription webhook guidance.
