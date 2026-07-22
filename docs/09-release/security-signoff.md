# Security Sign-off — `0.1.0-rc.1`

Decision: **Approved for Release Candidate; not approved for General Availability**.

Reviewer: `sessionweft-automation`  
Review type: automated security control and test review  
Date: 2026-07-22

## Evidence reviewed

- threat model and trust boundaries;
- default-deny tool policy and scoped approvals;
- one-time MCP approval consumption;
- bubblewrap filesystem/network/environment policy;
- malicious plugin fixtures;
- canonical workspace and symlink boundaries;
- fencing-token checks before Git mutations;
- constant-time bearer token comparison;
- tracked-source secret scan;
- dependency advisory/source/license policy;
- SBOM, checksums and provenance-attestation workflow.

## Findings

- Open Critical findings: 0.
- Open High findings: 0.
- No tested control permits stale lock ownership, duplicate event side effects, plugin secret access or workspace escape.
- GA remains blocked until a human security reviewer validates the deployment-specific sandbox, secret manager, TLS/network policy and final dependency audit output.
