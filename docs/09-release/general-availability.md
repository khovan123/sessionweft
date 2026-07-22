# General Availability — SessionWeft 0.1.0

Status: **Approved for General Availability within the declared scope**.  
Approval date: 2026-07-22  
Approving authority: `khovan123`  
Analysis executor: `sessionweft-automation`

## Decision

The repository owner delegated Architecture, Security and Operations research and evidence analysis to automation, reviewed the resulting decision model and authorized the GA approvals to be applied when all release-blocking gates pass.

The approval is represented transparently:

- the human authority is `khovan123`;
- automation is the analysis executor;
- the GA evidence identifies the exact CI-tested commit at runtime;
- no failed gate, Critical finding or High finding can be waived by the delegation.

## Approved product scope

- SQLite local single-user Runtime mode.
- Authenticated single-tenant service mode with PostgreSQL and NATS JetStream.
- Runtime-owned Session, Workflow, Agent, Memory, Lock, Git, Provider, Tool and event state.
- CLI, TUI and VS Code adapters.
- Linux production MCP/plugin sandbox using bubblewrap.

The exclusions in `ga-authorization.md` remain release constraints.

## GA evidence

- Policy: `release/ga-policy-0.1.0.json`
- Template: `release/evidence/ga-0.1.0.json`
- Exact-commit materialization: `scripts/release/materialize-evidence.py`
- Gate: `.github/workflows/ga-approval.yml`
- Architecture review: `ga-architecture-review.md`
- Security review: `ga-security-review.md`
- Operations review: `ga-operations-review.md`

## Verification command

```bash
python3 scripts/release/materialize-evidence.py \
  --template release/evidence/ga-0.1.0.json \
  --output release/evidence/ga-verified.json \
  --commit "$(git rev-parse HEAD)"

cargo run -p sessionweft-release-gate --locked -- \
  --policy release/ga-policy-0.1.0.json \
  --evidence release/evidence/ga-verified.json \
  --level ga
```

A GA tag such as `v0.1.0` is packaged only after the GA gate passes for the tagged commit. The release workflow generates checksums, a CycloneDX SBOM and GitHub provenance/SBOM attestations.

Related issue: #64.
