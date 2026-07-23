# General Availability — SessionWeft 0.2.0

Status: **Approved for General Availability after the exact-commit 0.2.0 gate passes.**  
Approving authority: `khovan123`  
Analysis executor: `sessionweft-automation`

## Scope added in 0.2.0

- Multi-tenant SaaS identity, membership, quota and tenant-isolated Runtime APIs.
- PostgreSQL tenant authority with FORCE ROW LEVEL SECURITY and transaction-local tenant context.
- Billing plans, subscriptions, entitlements and append-only usage authority.
- Idempotent Stripe customer, subscription, meter-event and webhook reference adapter.
- Portable Wasmtime plugin isolation on Linux, macOS and Windows.
- Exact-commit certification and activation enforcement for provider, plugin, deployment and billing adapters.
- Release packages containing adapter manifests, verified certifications, activation policy and source evidence.

## Architecture review

The tenant Runtime manager selects an isolated PostgreSQL schema only after tenant authentication and membership resolution. Durable product and entitlement state remains Runtime-owned. Billing providers are external side-effect adapters and cannot directly grant product entitlements. Adapter activation is fail closed and bound to the exact packaged commit.

Decision: **Approved for GA** when tenant isolation, quota, billing, sandbox and adapter-certification gates pass on the exact release commit.

## Security review

- tenant tokens are stored only as SHA-256 digests and raw tokens are returned once;
- bootstrap tokens use constant-time comparison;
- cross-tenant resources use not-found semantics;
- Runtime database roles cannot bypass row security;
- usage and webhook records are idempotent and auditable;
- portable plugins receive no filesystem, network, process, clock, environment or secret imports;
- Wasmtime execution is bounded by integrity digest, fuel, epoch timeout, memory, stack, input and output limits;
- uncertified or commit-mismatched adapters cannot be activated.

Open Critical findings: `0`.  
Open High findings: `0`.

Decision: **Approved for GA** when security-supply-chain and Phase 3 qualification pass without waiver.

## Operations review

- tenant Runtime restart preserves isolated Session state;
- quota reservations and billing usage survive retries without duplication;
- raw Stripe webhook ingestion is deduplicated before processing;
- release packages record build commit, release gate, adapter activation and certification paths;
- the 0.2.0 publication workflow emits checksums, CycloneDX SBOM, exact-commit evidence and provenance attestations;
- `v0.1.0` remains immutable and is not moved or replaced.

Decision: **Approved for GA** when CI, SaaS Runtime, service mode, production hardening, Phase 3 qualification, security and exact-commit 0.2.0 gates pass.

## Evidence

- Policy: `release/ga-policy-0.2.0.json`
- Evidence template: `release/evidence/ga-0.2.0.json`
- Phase 3 gate: `.github/workflows/phase3-qualification.yml`
- SaaS integration gate: `.github/workflows/saas-runtime.yml`
- Exact-commit GA gate: `.github/workflows/ga-0.2.0.yml`
- Publication: `.github/workflows/publish-v0.2.0.yml`
- Publication verification: `.github/workflows/verify-v0.2.0-publication.yml`

Related: #66.
