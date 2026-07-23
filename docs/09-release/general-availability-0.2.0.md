# General Availability — SessionWeft 0.2.0

Status: **Approved for General Availability when every exact-commit gate passes**.  
Approval date: 2026-07-23  
Approving authority: `khovan123`  
Analysis executor: `sessionweft-automation`

## Decision

SessionWeft 0.2.0 expands the 0.1.0 General Availability scope only after tenant isolation, billing idempotence, portable plugin isolation and Runtime adapter activation are verified on the exact release commit.

The approval remains fail-closed:

- the human authority is `khovan123`;
- automation executes checks and materializes evidence but cannot waive a failed gate;
- evidence must identify the exact commit referenced by `v0.2.0`;
- no Critical or High security finding may remain open;
- every production adapter must match a verified certification by ID, version, kind and build commit.

## GA scope added in 0.2.0

- Multi-tenant SaaS Runtime with tenant-owned identities, memberships, quotas and isolated PostgreSQL schemas.
- Tenant-scoped Session, Agent, Workflow and Lock APIs with cross-tenant not-found semantics.
- Hashed tenant API tokens, one-time raw token issuance and token revocation.
- Billing plans, subscriptions, entitlements, immutable usage records and idempotent Stripe integration.
- Portable Wasmtime/WASI plugin isolation on Linux, macOS and Windows.
- Exact-commit production activation for provider, plugin, deployment and billing adapters.
- Release packages containing adapter manifests, verified certifications, activation policy and source evidence.

The 0.1.0 local and authenticated single-tenant service modes remain supported.

## Safety invariants

- Durable tenant state is isolated by tenant schema and default-deny authority checks.
- Runtime database roles cannot bypass required row-level security controls.
- Billing providers never own SessionWeft entitlement state.
- Plugin capabilities remain absent unless explicitly granted.
- A missing, stale, wrong-kind or wrong-commit adapter certification prevents Runtime activation.
- Release evidence, checksums, SBOM and provenance are generated from the tagged commit.

## GA evidence

- Policy: `release/ga-policy-0.2.0.json`
- Template: `release/evidence/ga-0.2.0.json`
- Exact-commit materialization: `scripts/release/materialize-evidence.py`
- Adapter activation policy: `release/adapters/activation.json`
- Phase 3 qualification: `.github/workflows/phase3-qualification.yml`
- SaaS Runtime qualification: `.github/workflows/saas-runtime.yml`
- GA gate: `.github/workflows/ga-approval.yml`
- Release workflow: `.github/workflows/release.yml`

## Verification command

```bash
python3 scripts/release/materialize-evidence.py \
  --template release/evidence/ga-0.2.0.json \
  --output release/evidence/ga-verified.json \
  --commit "$(git rev-parse HEAD)"

cargo run -p sessionweft-release-gate --locked -- \
  --policy release/ga-policy-0.2.0.json \
  --evidence release/evidence/ga-verified.json \
  --level ga
```

The `v0.2.0` tag may be created only after all required workflows report success for the same main-branch commit. Publication verification then checks release assets, checksums, CycloneDX SBOM, exact-commit evidence and packaged adapter certifications.

Related issue: #66.
